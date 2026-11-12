//! Budget admission drives the same content transfer that generation uses.
use super::*;
use async_trait::async_trait;
use orch_link::content::{BlobEndpoint, PutRefusal};
use orch_link::framing::LinkMeter;
use orch_link_contract::commands::NativeCapabilityInput;
use orch_link_contract::content::{BlobProgress, PutQuery};
use orch_model_contract::endpoint::EndpointOptions;
use orch_model_contract::generation::{
    GenerationInput, GenerationMessage, GenerationPart, GenerationRole,
};
use orch_types::{Hash, new_id};
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct CountingBlobs {
    bytes: Bytes,
    reads: AtomicUsize,
}
impl CountingBlobs {
    fn new(bytes: &'static [u8]) -> Arc<Self> {
        Arc::new(Self {
            bytes: Bytes::from_static(bytes),
            reads: AtomicUsize::new(0),
        })
    }
}
#[async_trait]
impl BlobEndpoint for CountingBlobs {
    async fn head(&self, _: Id, _: &Hash) -> Result<Option<BlobProgress>, PutRefusal> {
        Err(PutRefusal::Refused("unexpected metadata request".into()))
    }
    async fn put(
        &self,
        _: Id,
        _: &Hash,
        _: PutQuery,
        _: Bytes,
    ) -> Result<BlobProgress, PutRefusal> {
        Err(PutRefusal::Refused("unexpected upload".into()))
    }
    async fn get(&self, _: Id, hash: &Hash, offset: i64, len: i64) -> Result<Bytes, PutRefusal> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let from = usize::try_from(offset).map_err(|_| PutRefusal::Refused("bad offset".into()))?;
        let len = usize::try_from(len).map_err(|_| PutRefusal::Refused("bad length".into()))?;
        if Hash::of(&self.bytes) != *hash || from >= self.bytes.len() {
            return Err(PutRefusal::Refused("missing blob".into()));
        }
        Ok(self
            .bytes
            .slice(from..from.saturating_add(len).min(self.bytes.len())))
    }
    async fn missing(&self, _: Id, _: &[Hash]) -> Result<Vec<Hash>, PutRefusal> {
        Err(PutRefusal::Refused("unexpected existence request".into()))
    }
}
fn protocols() -> Vec<Box<dyn WireProtocol>> {
    vec![
        Box::new(orch_protocol_openai_responses::OpenAiResponses),
        Box::new(orch_protocol_openai_responses::OpenAiChat),
        Box::new(orch_protocol_anthropic_messages::AnthropicMessages),
    ]
}
fn request(
    protocol: &dyn WireProtocol,
    bytes: &[u8],
    size: i64,
    required: bool,
) -> ProviderGenerate {
    ProviderGenerate {
        native_capability: required.then(|| NativeCapabilityInput {
            capability: "web_search".into(),
            input: serde_json::json!({}),
            definition: Hash::of(b"validated declaration"),
        }),
        capture_native_result: required,
        provider_id: new_id(),
        protocol: protocol.name().into(),
        model_name: "image-model".into(),
        params: None,
        model_parameters: orch_provider_contract::models::ModelParameters {
            input_images: Some(true),
            ..Default::default()
        },
        input: GenerationInput {
            system: String::new(),
            messages: vec![GenerationMessage {
                role: GenerationRole::User,
                continuation: vec![],
                content: vec![GenerationPart::ImageRef {
                    blob: Hash::of(bytes),
                    size,
                    media_type: "image/png".into(),
                    name: "image".into(),
                    url: "u".into(),
                }],
            }],
            tools: vec![],
            max_output_tokens: 16,
        },
    }
}
fn content(blobs: &Arc<CountingBlobs>, scratch: &std::path::Path) -> crate::content::Content {
    crate::content::Content::new(blobs.clone(), LinkMeter::new(), scratch.to_owned())
}
fn is_link(part: &GenerationPart) -> bool {
    matches!(part, GenerationPart::Text { text } if text == "[image](u)")
}
fn empty_image() -> GenerationPart {
    GenerationPart::Image {
        media_type: "image/png".into(),
        data: String::new(),
        name: "image".into(),
    }
}

#[tokio::test]
async fn an_oversized_reference_is_never_fetched_and_required_images_keep_their_budget_error()
-> TestResult {
    let scratch = tempfile::tempdir()?;
    for protocol in protocols() {
        for required in [false, true] {
            for size in [4096_i64, i64::MAX] {
                let blobs = CountingBlobs::new(b"abc");
                let content = content(&blobs, scratch.path());
                let command = new_id();
                let mut request = request(protocol.as_ref(), b"abc", size, required);
                let result = materialize_images(
                    &content,
                    protocol.as_ref(),
                    &EndpointOptions::default(),
                    None,
                    &mut request,
                    command,
                    1024,
                )
                .await;
                if required {
                    let error = result.err().ok_or("required image did not fail")?;
                    assert_eq!(error.known(), Some(ErrorCode::TooLarge));
                    assert_eq!(
                        error.detail,
                        "a required capability input image exceeds the request budget"
                    );
                } else {
                    result?;
                    assert!(is_link(&request.input.messages[0].content[0]));
                }
                assert_eq!(blobs.reads.load(Ordering::SeqCst), 0);
                assert!(
                    !content.directory_of(command).exists(),
                    "no scratch file or transfer was started"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn request_overhead_and_existing_images_spend_the_allowance_before_the_next_blob_is_fetched()
-> TestResult {
    let scratch = tempfile::tempdir()?;
    for protocol in protocols() {
        let blobs = CountingBlobs::new(b"abc");
        let content = content(&blobs, scratch.path());
        let options = EndpointOptions::default();
        for required in [false, true] {
            let mut request = request(protocol.as_ref(), b"abc", 3, required);
            let mut projection = request.clone();
            projection.input.messages[0].content[0] = empty_image();
            let (_, _, envelope) =
                build_generation_with_auxiliary(protocol.as_ref(), &options, &projection, None)?;
            // Three raw bytes need four base64 bytes; only three remain after the real envelope.
            let allowance = u64::try_from(envelope.len())? + 3;
            let result = materialize_images(
                &content,
                protocol.as_ref(),
                &options,
                None,
                &mut request,
                new_id(),
                allowance,
            )
            .await;
            if required {
                assert_eq!(
                    result.err().ok_or("required image did not fail")?.known(),
                    Some(ErrorCode::TooLarge)
                );
            } else {
                result?;
                assert!(is_link(&request.input.messages[0].content[0]));
            }
            assert_eq!(blobs.reads.load(Ordering::SeqCst), 0);
        }
        let mut request = request(protocol.as_ref(), b"abc", 3, false);
        let second = request.input.messages[0].content[0].clone();
        request.input.messages[0].content.push(second);
        let mut projection = request.clone();
        projection.input.messages[0].content[0] = GenerationPart::Image {
            media_type: "image/png".into(),
            data: "YWJj".into(),
            name: "image".into(),
        };
        let (_, _, first_body) =
            build_generation_with_auxiliary(protocol.as_ref(), &options, &projection, None)?;
        materialize_images(
            &content,
            protocol.as_ref(),
            &options,
            None,
            &mut request,
            new_id(),
            u64::try_from(first_body.len())?,
        )
        .await?;
        assert!(
            matches!(&request.input.messages[0].content[0], GenerationPart::Image { data, .. } if data == "YWJj")
        );
        assert!(is_link(&request.input.messages[0].content[1]));
        assert_eq!(
            blobs.reads.load(Ordering::SeqCst),
            1,
            "only the first image fits the cumulative request"
        );
    }
    Ok(())
}

#[tokio::test]
async fn admitted_images_keep_padding_wire_size_and_original_content_transfer() -> TestResult {
    let scratch = tempfile::tempdir()?;
    for protocol in protocols() {
        for bytes in [b"a".as_slice(), b"ab", b"abc", b"abcd"] {
            // The endpoint owns static test bytes, as the actual retained store does.
            let blobs = CountingBlobs::new(bytes);
            let content = content(&blobs, scratch.path());
            let options = EndpointOptions::default();
            let mut request = request(protocol.as_ref(), bytes, i64::try_from(bytes.len())?, false);
            let mut projection = request.clone();
            projection.input.messages[0].content[0] = empty_image();
            let (_, _, envelope) =
                build_generation_with_auxiliary(protocol.as_ref(), &options, &projection, None)?;
            let encoded = u64::try_from(bytes.len())?.div_ceil(3) * 4;
            let allowance = u64::try_from(envelope.len())? + encoded;
            materialize_images(
                &content,
                protocol.as_ref(),
                &options,
                None,
                &mut request,
                new_id(),
                allowance,
            )
            .await?;
            assert!(matches!(
                request.input.messages[0].content[0],
                GenerationPart::Image { .. }
            ));
            let (_, _, actual) =
                build_generation_with_auxiliary(protocol.as_ref(), &options, &request, None)?;
            assert_eq!(u64::try_from(actual.len())?, allowance);
            assert_eq!(blobs.reads.load(Ordering::SeqCst), 1);
        }
    }
    Ok(())
}

#[tokio::test]
async fn missing_images_and_invalid_sizes_keep_unavailable_distinct_from_too_large() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let protocol = orch_protocol_openai_responses::OpenAiResponses;
    for size in [-1, 3] {
        for required in [false, true] {
            let blobs = CountingBlobs::new(b"another blob");
            let content = content(&blobs, scratch.path());
            let mut request = request(&protocol, b"abc", size, required);
            let result = materialize_images(
                &content,
                &protocol,
                &EndpointOptions::default(),
                None,
                &mut request,
                new_id(),
                4096,
            )
            .await;
            if required {
                let error = result.err().ok_or("required image did not fail")?;
                assert_eq!(error.known(), Some(ErrorCode::Conflict));
                assert_eq!(
                    error.detail,
                    "a required capability input image is unavailable"
                );
            } else {
                result?;
                assert!(is_link(&request.input.messages[0].content[0]));
            }
            assert_eq!(blobs.reads.load(Ordering::SeqCst), usize::from(size >= 0));
        }
    }
    let changed = scratch.path().join("changed-image");
    tokio::fs::write(&changed, b"more than the admitted size").await?;
    assert!(read_image_bytes(&changed, 3).await.is_none());
    Ok(())
}
