#![allow(
    clippy::unwrap_used,
    reason = "malformed recorded fixture data fails the test"
)]

use super::*;
use orch_provider_contract::capabilities::{Registry, catalogue, project_endpoint, render_request};
use serde_json::json;

// Sanitized recorded reply shapes with a fixture image; no provider/key/network is involved.
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6B9sAAAAASUVORK5CYII=";

fn limits() -> DecodeLimits {
    DecodeLimits {
        images: 10,
        image_bytes: 1024,
        total_bytes: 4096,
    }
}

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry.register("common", catalogue().unwrap()).unwrap();
    for (name, declarations) in declarations().unwrap() {
        registry.register(name, declarations).unwrap();
    }
    registry
}

fn compatible() -> Binding {
    registry().declarations().interfaces["openai-images"]
        .shape
        .clone()
}

#[test]
fn common_projected_image_rows_are_consumed_once_for_each_declared_wire_shape() {
    let registry = registry();
    let hosted =
        registry.declarations().interfaces["openai-responses"].native_tools["image_generation"]
            .clone();
    let dedicated = registry.declarations().interfaces["openrouter-images"]
        .shape
        .clone();
    for (binding, response) in [
        (
            compatible(),
            json!({"data":[{"b64_json":PNG,"revised_prompt":"compatible"}]}),
        ),
        (
            hosted,
            json!({"status":"completed","output":[{"type":"image_generation_call","status":"completed","id":"ig_common","result":PNG,"revised_prompt":"hosted"}]}),
        ),
        (
            dedicated,
            json!({"data":[{"b64_json":PNG,"media_type":"image/png"}],"usage":{"cost":0.0123}}),
        ),
    ] {
        let normalized = project_result(&binding, &json!({}), &response, limits().images).unwrap();
        let consumed = decode_projected(&normalized, limits()).unwrap();
        let complete = decode(&binding, &json!({}), &response, limits()).unwrap();
        assert_eq!(consumed, complete.images);
        assert_eq!(consumed.len(), 1);
        assert!(!consumed[0].bytes.is_empty());
    }
}

#[test]
fn projected_images_keep_multiplicity_and_unknown_media_and_refuse_unbounded_or_url_only_results() {
    let normalized = json!([{"b64_json":PNG},{"b64_json":PNG}]);
    let images = decode_projected(&normalized, limits()).unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!((images[0].ordinal, images[1].ordinal), (0, 1));
    assert_eq!(images[0].bytes, images[1].bytes);
    assert!(images.iter().all(|image| image.media_type.is_none()));
    assert!(
        decode_projected(
            &normalized,
            DecodeLimits {
                images: 1,
                ..limits()
            }
        )
        .is_err()
    );
    assert!(
        decode_projected(
            &normalized,
            DecodeLimits {
                image_bytes: 1,
                ..limits()
            }
        )
        .is_err()
    );
    assert!(
        decode_projected(
            &normalized,
            DecodeLimits {
                total_bytes: 1,
                ..limits()
            }
        )
        .is_err()
    );
    for invalid in [
        json!([]),
        json!({"data":[{"b64_json":PNG}]}),
        json!([{"url":"https://untrusted.invalid/never-follow"}]),
        json!([{"b64_json":PNG},{"b64_json":"A==="}]),
        json!([{"b64_json":PNG,"media_type":"text/html"}]),
    ] {
        assert!(decode_projected(&invalid, limits()).is_err());
    }
}

#[test]
fn compatible_outputs_preserve_identical_images_and_actual_usage_without_fetching_returned_urls() {
    let response = json!({"data":[
        {"b64_json":PNG,"revised_prompt":"fixture","url":"https://untrusted.invalid/never-follow"},
        {"b64_json":PNG,"revised_prompt":"fixture","url":"https://untrusted.invalid/never-follow"}
    ],"usage":{"input_tokens":17,"output_tokens":91,"output_tokens_details":{"image_tokens":88}}});
    let result = decode(&compatible(), &json!({}), &response, limits()).unwrap();
    assert_eq!(
        result.images.len(),
        2,
        "two outputs are two images even when their bytes match"
    );
    assert_eq!(result.images[0].bytes, result.images[1].bytes);
    assert_eq!(result.images[0].ordinal, 0);
    assert_eq!(result.images[1].ordinal, 1);
    assert!(
        result.images[0].media_type.is_none(),
        "an absent reported type does not become PNG"
    );
    assert_eq!(result.images[0].revised_prompt.as_deref(), Some("fixture"));
    assert_eq!(result.usage, response["usage"]);
    assert!(
        result.usage.get("cost").is_none(),
        "counts do not invent a price"
    );
}

#[test]
fn hosted_final_calls_keep_native_identity_and_revised_prompt_and_reject_partial_or_failed_results()
{
    let binding =
        registry().declarations().interfaces["openai-responses"].native_tools["image_generation"]
            .clone();
    let response = json!({"status":"completed","output":[
        {"type":"message","content":[]},
        {"type":"image_generation_call","status":"completed","id":"ig_a","result":PNG,"revised_prompt":"first"},
        {"type":"image_generation_call","status":"completed","id":"ig_b","result":PNG,"revised_prompt":"second"}
    ],"usage":{"input_tokens":10,"output_tokens":200}});
    let decoded = decode(&binding, &json!({}), &response, limits()).unwrap();
    assert_eq!(decoded.images.len(), 2);
    assert_eq!(decoded.images[0].native_ref.as_deref(), Some("ig_a"));
    assert_eq!(decoded.images[1].native_ref.as_deref(), Some("ig_b"));
    assert_eq!(decoded.images[1].revised_prompt.as_deref(), Some("second"));
    assert!(
        decode(
            &binding,
            &json!({}),
            &json!({"status":"in_progress","output":response["output"]}),
            limits()
        )
        .is_err()
    );
    assert!(decode(&binding, &json!({}), &json!({"status":"completed","output":[{"type":"image_generation_call","status":"in_progress","id":"ig_partial","result":PNG}]}), limits()).is_err());
    assert!(
        decode(
            &binding,
            &json!({}),
            &json!({"status":"failed","error":{"code":"refused"},"output":response["output"]}),
            limits()
        )
        .is_err()
    );
}

#[test]
fn a_malformed_completed_image_cannot_disappear_beside_a_valid_native_output() {
    let binding =
        registry().declarations().interfaces["openai-responses"].native_tools["image_generation"]
            .clone();
    let valid =
        json!({"type":"image_generation_call","status":"completed","id":"ig_valid","result":PNG});
    let unrelated = json!({"type":"message","content":[{"type":"output_text","text":"fixture"}]});
    let ordinary = json!({"status":"completed","output":[unrelated,valid.clone()],"usage":{"input_tokens":11,"output_tokens":88}});
    let positive = decode(&binding, &json!({}), &ordinary, limits()).unwrap();
    assert_eq!(positive.images.len(), 1);
    assert_eq!(positive.images[0].native_ref.as_deref(), Some("ig_valid"));
    assert_eq!(positive.usage, ordinary["usage"]);
    for invalid in [
        json!({"type":"image_generation_call","status":"completed","result":PNG}),
        json!({"type":"image_generation_call","status":"completed","id":"ig_bad"}),
        json!({"type":"image_generation_call","status":"completed","id":"","result":PNG}),
        json!({"type":"image_generation_call","status":"completed","id":"ig_bad","result":""}),
        json!({"type":"image_generation_call","status":"completed","id":"ig_bad","result":17}),
    ] {
        for output in [
            json!([valid.clone(), invalid.clone()]),
            json!([invalid, valid.clone()]),
        ] {
            let response = json!({"status":"completed","output":output,"usage":ordinary["usage"]});
            assert!(
                decode(&binding, &json!({}), &response, limits()).is_err(),
                "malformed completed evidence must not be filtered into a successful smaller result"
            );
        }
    }
}

#[test]
fn dedicated_api_retains_reported_cost_and_media_without_estimating_or_adding_to_it() {
    let binding = registry().declarations().interfaces["openrouter-images"]
        .shape
        .clone();
    let response = json!({"created":1748372400,"data":[{"b64_json":PNG,"media_type":"image/png"}],
        "usage":{"prompt_tokens":16,"completion_tokens":272,"total_tokens":288,"cost":0.011}});
    let result = decode(&binding, &json!({}), &response, limits()).unwrap();
    assert_eq!(result.images[0].media_type.as_deref(), Some("image/png"));
    assert_eq!(result.usage, response["usage"]);
    assert_eq!(binding.counters["input_tokens"].count(&response), Some(16));
    assert!(binding.counters["reported_cost"].reported_cost);
    assert_eq!(binding.counters["reported_cost"].pointer, "/usage/cost");
    assert!(
        decode(
            &binding,
            &json!({}),
            &json!({"error":{"message":"failed"},"data":[]}),
            limits()
        )
        .is_err()
    );
}

#[test]
fn decoding_obeys_actual_byte_and_count_limits_before_allocation_and_keeps_unknown_usage_unknown() {
    let binding = compatible();
    let exact = DecodeLimits {
        images: 1,
        image_bytes: 1,
        total_bytes: 1,
    };
    let result = decode(
        &binding,
        &json!({}),
        &json!({"data":[{"b64_json":"AA=="}]}),
        exact,
    )
    .unwrap();
    assert_eq!(result.images[0].bytes, [0]);
    assert!(result.usage.is_null());
    let identified = decode(
        &binding,
        &json!({}),
        &json!({"output_format":"png","data":[{"b64_json":"AA=="}]}),
        exact,
    )
    .unwrap();
    assert_eq!(
        identified.images[0].media_type.as_deref(),
        Some("image/png"),
        "only an actually reported format may supply its declared media type"
    );
    for response in [
        json!({"data":[{"b64_json":"AA=="},{"b64_json":"AA=="}]}),
        json!({"data":[{"b64_json":"AAA="}]}),
        json!({"data":[{"b64_json":"!!!!"}]}),
        json!({"data":[{"b64_json":"AA="}]}),
        json!({"data":[{"url":"https://untrusted.invalid/image.png"}]}),
        json!({"data":[{"b64_json":"AA==","media_type":"text/html"}]}),
        json!({"data":[]}),
    ] {
        assert!(decode(&binding, &json!({}), &response, exact).is_err());
    }
    let limited_total = DecodeLimits {
        images: 2,
        image_bytes: 1,
        total_bytes: 1,
    };
    assert!(
        decode(
            &binding,
            &json!({}),
            &json!({"data":[{"b64_json":"AA=="},{"b64_json":"AA=="}]}),
            limited_total
        )
        .is_err()
    );
}

#[test]
fn endpoint_offers_remain_disabled_and_compatible_accounts_do_not_inherit_each_others_options() {
    let mut registry = registry();
    let full = registry.declarations().clone();
    let offered = project_endpoint(
        &full,
        "openai-responses",
        &url::Url::parse("https://api.openai.com/v1").unwrap(),
        "gpt-image-2",
    );
    assert!(!offered.endpoints.is_empty());
    assert!(offered.offerings.values().all(|offering| !offering.enabled));
    assert!(
        offered.offerings["gpt-image-2"].options["properties"]
            .get("response_format")
            .is_none()
    );
    let base = url::Url::parse("https://generativelanguage.googleapis.com/v1beta/openai/").unwrap();
    let google = project_endpoint(
        registry.declarations(),
        "openai-chat",
        &base,
        "gemini-2.5-flash-image",
    );
    assert_eq!(google.endpoints["images"].interface, "openai-images");
    assert!(
        google
            .offerings
            .values()
            .all(|offering| !offering.enabled && offering.kind == "service")
    );
    assert!(
        google.offerings["gemini-2.5-flash-image"].options["properties"]
            .get("quality")
            .is_none()
    );
    assert_eq!(
        google.offerings["gemini-2.5-flash-image"].options["properties"]["response_format"]["enum"],
        json!(["b64_json"])
    );
    assert!(registry.remove("images.openai"));
    assert_eq!(
        project_endpoint(
            registry.declarations(),
            "openai-chat",
            &base,
            "gemini-2.5-flash-image"
        ),
        google,
        "removing an account extension does not remove a shared wire interface"
    );
    assert!(
        registry
            .declarations()
            .interfaces
            .contains_key("openai-images")
    );
    for address in [
        "https://api.openai.com.attacker.invalid/v1",
        "https://user@api.openai.com/v1",
        "https://api.openai.com:8443/v1",
        "https://api.openai.com/v1?redirect=elsewhere",
        "http://api.openai.com/v1",
    ] {
        assert!(
            project_endpoint(
                &full,
                "openai-responses",
                &url::Url::parse(address).unwrap(),
                "gpt-image-2"
            )
            .endpoints
            .is_empty()
        );
    }
}

#[test]
fn requests_use_the_declared_shape_and_keep_literals_and_missing_optional_fields_unchanged() {
    let binding = compatible();
    let input = json!({"model":"fixture-model","prompt":"{\"from_response\":\"/secret\"}","n":2});
    let request = render_request(&binding, &input).unwrap();
    assert_eq!(request["path"], "/images/generations");
    assert_eq!(request["body"], input);
    assert!(
        request["body"].get("response_format").is_none(),
        "do not inject a parameter unsupported by GPT Image"
    );
    assert!(
        render_request(
            &binding,
            &json!({"model":"fixture-model","prompt":"fixture","n":11})
        )
        .is_err()
    );
    let edits: Binding =
        serde_json::from_value(binding.extensions["x_actions"]["edits"].clone()).unwrap();
    assert!(render_request(&edits, &input).is_err());
    let edited = render_request(
        &edits,
        &json!({"model":"fixture-model","prompt":"edit","image":"retained-file-reference"}),
    )
    .unwrap();
    assert_eq!(edited["path"], "/images/edits");
    assert_eq!(edited["encoding"], "multipart");
    let openai = project_endpoint(
        registry().declarations(),
        "openai-responses",
        &url::Url::parse("https://api.openai.com/v1").unwrap(),
        "gpt-image-2",
    );
    assert!(
        openai.offerings["gpt-image-2"].options["properties"]
            .get("image")
            .is_some(),
        "the final offering constraint must permit the declared edit input"
    );
    let dedicated = registry().declarations().interfaces["openrouter-images"]
        .shape
        .clone();
    assert_eq!(
        dedicated.discovery["endpoint_request"]["path_template"],
        "/images/models/{id}/endpoints"
    );
    assert_eq!(
        dedicated.discovery["definitive_options"],
        "/supported_parameters"
    );
    assert_eq!(dedicated.discovery["enabled"], json!(false));
}

#[test]
fn streamed_previews_do_not_complete_a_call_and_the_same_final_emits_once() {
    let binding = registry().declarations().interfaces["openrouter-images"]
        .shape
        .clone();
    let mut stream = ImageStream::new(binding.clone(), json!({}), limits());
    assert!(
        stream
            .accept(json!({"type":"progress","percent":10}))
            .unwrap()
            .is_none()
    );
    let preview = stream
        .accept(
            json!({"type":"image_generation.partial_image","partial_image_index":0,"b64_json":PNG}),
        )
        .unwrap()
        .unwrap();
    let ImageEvent::Preview { index, images } = preview else {
        panic!("a preview is not final evidence")
    };
    assert_eq!(index, Some(0));
    assert_eq!(images.images.len(), 1);
    assert!(images.usage.is_null());
    let final_event = json!({"type":"image_generation.completed","b64_json":PNG,"media_type":"image/png","usage":{"cost":0.011}});
    let result = stream.accept(final_event.clone()).unwrap().unwrap();
    let ImageEvent::Completed(images) = result else {
        panic!("a final response is completed")
    };
    assert_eq!(images.usage, json!({"cost":0.011}));
    assert_eq!(images.images.len(), 1);
    assert!(
        stream.accept(final_event).unwrap().is_none(),
        "a repeated final cannot produce or charge again"
    );
    assert!(
        stream
            .accept(json!({"type":"image_generation.completed","b64_json":"AA=="}))
            .is_err()
    );
    assert!(
        stream.finish().is_ok(),
        "a later anomalous frame does not erase accepted output"
    );
    assert_eq!(images.images[0].media_type.as_deref(), Some("image/png"));

    let mut failed = ImageStream::new(binding, json!({}), limits());
    assert!(
        failed
            .accept(json!({"type":"error","error":{"code":"server_error"}}))
            .is_err()
    );
    assert!(
        failed
            .accept(json!({"type":"image_generation.completed","b64_json":PNG}))
            .is_err()
    );
    assert!(failed.finish().is_err());
}

#[test]
fn eof_without_final_evidence_cannot_be_reopened_by_late_completed_frames() {
    let registry = registry();
    let dedicated = registry.declarations().interfaces["openrouter-images"]
        .shape
        .clone();
    let hosted =
        registry.declarations().interfaces["openai-responses"].native_tools["image_generation"]
            .clone();
    for (binding, preview, final_event) in [
        (
            dedicated,
            json!({"type":"image_generation.partial_image","partial_image_index":0,"b64_json":PNG}),
            json!({"type":"image_generation.completed","b64_json":PNG,"usage":{"cost":0.011}}),
        ),
        (
            hosted,
            json!({"type":"response.image_generation_call.partial_image","partial_image_index":0,"partial_image_b64":PNG}),
            json!({"type":"response.completed","response":{"status":"completed","output":[
                {"type":"image_generation_call","status":"completed","id":"ig_late","result":PNG}
            ],"usage":{"input_tokens":19,"output_tokens":91}}}),
        ),
    ] {
        for with_preview in [false, true] {
            let mut stream = ImageStream::new(binding.clone(), json!({}), limits());
            if with_preview {
                let ImageEvent::Preview { images, .. } =
                    stream.accept(preview.clone()).unwrap().unwrap()
                else {
                    panic!("intermediate bytes remain a preview")
                };
                assert!(images.usage.is_null());
            }
            assert!(stream.finish().is_err(), "EOF requires real final evidence");
            assert!(
                stream.accept(final_event.clone()).is_err(),
                "late evidence cannot complete an invocation that ended incomplete"
            );
            assert!(
                stream.finish().is_err(),
                "the incomplete end stays terminal"
            );
        }
    }
}

#[test]
fn hosted_streams_use_the_declared_nested_final_response_and_keep_preview_usage_unknown() {
    let binding =
        registry().declarations().interfaces["openai-responses"].native_tools["image_generation"]
            .clone();
    let mut stream = ImageStream::new(binding, json!({}), limits());
    let preview = stream.accept(json!({"type":"response.image_generation_call.partial_image","partial_image_index":1,"partial_image_b64":PNG})).unwrap().unwrap();
    let ImageEvent::Preview { index, images } = preview else {
        panic!("the hosted partial remains a preview")
    };
    assert_eq!(index, Some(1));
    assert!(
        images.usage.is_null(),
        "unknown partial token cost is not zero/free"
    );
    let final_event = json!({"type":"response.completed","response":{"status":"completed","output":[
        {"type":"image_generation_call","status":"completed","id":"ig_actual","result":PNG,"revised_prompt":"actual"}
    ],"usage":{"input_tokens":19,"output_tokens":91}}});
    let ImageEvent::Completed(result) = stream.accept(final_event.clone()).unwrap().unwrap() else {
        panic!("completed response expected")
    };
    assert_eq!(result.images[0].native_ref.as_deref(), Some("ig_actual"));
    assert_eq!(result.usage, final_event["response"]["usage"]);
    assert!(stream.accept(final_event).unwrap().is_none());
    assert!(stream.finish().is_ok());
}

#[test]
fn malformed_stream_evidence_is_terminal_before_completion_without_discarding_accepted_output() {
    let registry = registry();
    let dedicated = registry.declarations().interfaces["openrouter-images"]
        .shape
        .clone();
    let hosted =
        registry.declarations().interfaces["openai-responses"].native_tools["image_generation"]
            .clone();
    let dedicated_final = json!({"type":"image_generation.completed","b64_json":PNG,"media_type":"image/png","usage":{"cost":0.011}});
    let hosted_final = json!({"type":"response.completed","response":{"status":"completed","output":[
        {"type":"image_generation_call","status":"completed","id":"ig_retained","result":PNG}
    ]}});
    for (binding, bad, valid) in [
        (
            hosted.clone(),
            json!({"type":"response.completed"}),
            hosted_final.clone(),
        ),
        (
            hosted,
            json!({"type":"response.image_generation_call.partial_image","partial_image_index":-1,"partial_image_b64":PNG}),
            hosted_final,
        ),
        (
            dedicated.clone(),
            json!({"type":"image_generation.partial_image","partial_image_index":"bad","b64_json":PNG}),
            dedicated_final.clone(),
        ),
        (
            dedicated.clone(),
            json!({"type":"image_generation.completed","b64_json":"!!!!"}),
            dedicated_final.clone(),
        ),
    ] {
        let mut stream = ImageStream::new(binding, json!({}), limits());
        assert!(stream.accept(bad).is_err());
        assert!(
            stream.accept(valid).is_err(),
            "another final cannot resurrect a malformed invocation"
        );
        assert!(stream.finish().is_err());
    }
    let mut stream = ImageStream::new(dedicated, json!({}), limits());
    let ImageEvent::Completed(retained) = stream.accept(dedicated_final.clone()).unwrap().unwrap()
    else {
        panic!("the first valid final is retained")
    };
    let bytes = retained.images[0].bytes.clone();
    assert!(
        stream
            .accept(json!({"type":"error","error":{"code":"trailing_error"}}))
            .is_err()
    );
    assert!(
        stream.finish().is_ok(),
        "a trailing error cannot revoke accepted completion"
    );
    assert_eq!(retained.images[0].bytes, bytes);
    assert_eq!(retained.usage, json!({"cost":0.011}));
    assert!(
        stream.accept(dedicated_final).unwrap().is_none(),
        "a retained final still emits only once"
    );
}

#[test]
fn inline_reference_images_follow_the_declared_data_url_shape_and_never_accept_remote_source_urls()
{
    let declarations = declarations().unwrap();
    let binding = declarations
        .iter()
        .find_map(|(_, declaration)| declaration.interfaces.get("openrouter-images"))
        .unwrap()
        .shape
        .clone();
    let source = InputImage {
        name: "source-1".into(),
        bytes: b"actual synthetic image bytes".to_vec(),
        media_type: "image/png".into(),
    };
    let input = json!({"model":"fixture-model","prompt":"An orchard","n":1});
    let result = materialize_inline_inputs(&binding, input.clone(), &[source]).unwrap();
    assert_eq!(result["model"], input["model"]);
    assert_eq!(result["input_references"][0]["type"], "image_url");
    assert_eq!(
        result["input_references"][0]["image_url"]["url"],
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"actual synthetic image bytes")
        )
    );
    assert!(result.get("image").is_none());
    assert!(result.get("images").is_none());
    let duplicate = || InputImage {
        name: "source-1".into(),
        bytes: vec![1],
        media_type: "image/png".into(),
    };
    assert!(
        materialize_inline_inputs(&binding, input.clone(), &[duplicate(), duplicate()]).is_err()
    );
    let overridden = json!({"model":"fixture-model","prompt":"An orchard","input_references":[{"image_url":{"url":"https://not-a-source.example.invalid/image"}}]});
    assert!(materialize_inline_inputs(&binding, overridden, &[duplicate()]).is_err());
    assert!(materialize_inline_inputs(&compatible(), input, &[duplicate()]).is_err());
}

#[test]
fn declared_output_counts_measure_decoded_finals_without_requested_counts_or_previews() {
    let api: Declarations = serde_json::from_str(include_str!("interfaces.json")).unwrap();
    let counter = &api.interfaces["openai-images"].shape.counters["image_generation"];
    assert_eq!(
        counter.count(&json!({"n":9,"data":[{"b64_json":"YQ=="},{"b64_json":"YQ=="}]})),
        Some(2)
    );
    assert_eq!(
        counter.count(&json!({"n":9,"data":[{"b64_json":"YQ=="},{"b64_json":"invalid"}]})),
        None
    );
    let native = orch_provider_contract::capabilities::protocol_interfaces().unwrap();
    let counter =
        &native["openai-responses"].native_tools["image_generation"].counters["image_generation"];
    assert_eq!(
        counter.count(&json!({"status":"completed","output":[
            {"type":"message","content":[]},
            {"type":"image_generation_call","status":"in_progress","result":"a preview"},
            {"type":"image_generation_call","status":"completed","result":"YQ=="},
            {"type":"image_generation_call","status":"completed","result":"YQ=="}
        ]})),
        Some(2)
    );
    assert_eq!(
        counter.count(&json!({"status":"in_progress","output":[]})),
        None
    );
}
