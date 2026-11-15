//! Bounded static file previews cached by content and served only through their authorized owner.

use crate::blobs::Blobs;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use axum::response::IntoResponse;

use axum::response::Response;

use base64::Engine as _;

use bytes::Bytes;

use http::HeaderValue;

use http::StatusCode;

use http::header;

use image::DynamicImage;

use image::ImageDecoder;

use image::ImageFormat;

use image::ImageReader;

use orch_blob_contract::range::ByteRange;

use orch_blob_contract::store::read_all;

use orch_settings::Place;

use orch_settings_contract::registry::keys;

use orch_types::Hash;

use serde::Serialize;

use std::collections::VecDeque;

use std::io::Cursor;

use std::future::Future;
/// Cached replies contain only derived bytes and metadata, never an original or an authority grant.
#[derive(Default)]
pub struct Cache {
    entries: VecDeque<(String, Bytes)>,
    bytes: usize,
}

impl Cache {
    fn take(&mut self, key: &str) -> Option<Bytes> {
        let at = self.entries.iter().position(|(held, _)| held == key)?;
        let entry = self.entries.remove(at)?;
        let value = entry.1.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    fn trim(&mut self, limit: usize) {
        while self.bytes > limit {
            let Some((_, value)) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(value.len());
        }
    }

    fn put(&mut self, key: String, value: Bytes, limit: usize) {
        if value.len() > limit {
            return;
        }
        self.trim(limit.saturating_sub(value.len()));
        self.bytes = self.bytes.saturating_add(value.len());
        self.entries.push_back((key, value));
    }
}

/// Effective operator settings; the registry owns their defaults and validation.
#[derive(Debug, Clone, Copy)]
struct Limits {
    defaults: [bool; 7],
    source: i64,
    pixels: u64,
    edge: u32,
    bytes: usize,
    cache: usize,
    lines: usize,
    viewer: i64,
}

impl Limits {
    async fn read(ctx: &Ctx) -> ApiResult<Self> {
        let mut conn = ctx.app.db.acquire().await?;
        let names = [
            keys::FILES_PREVIEW_SOURCE_BYTES,
            keys::FILES_PREVIEW_PIXELS,
            keys::FILES_PREVIEW_EDGE,
            keys::FILES_PREVIEW_BYTES,
            keys::FILES_PREVIEW_CACHE_BYTES,
            keys::REFERENCES_PREVIEW_LINES,
            keys::FILES_VIEWER_BYTES,
        ];
        let definitions = names
            .iter()
            .map(|key| {
                orch_settings_contract::registry::get(key)
                    .ok_or_else(|| ApiError::internal("a preview setting has no definition"))
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let snapshot = orch_settings::Snapshot::load(
            &mut conn,
            Place::System,
            orch_settings::Wanted::Keys(&definitions),
        )
        .await
        .map_err(ApiError::internal)?;
        let get = |key| -> ApiResult<i64> {
            let definition = orch_settings_contract::registry::get(key)
                .ok_or_else(|| ApiError::internal("a preview setting has no definition"))?;
            snapshot
                .resolve(definition)
                .value
                .and_then(|value| value.as_i64())
                .ok_or_else(|| ApiError::internal("a preview setting has no integer default"))
        };
        let defaults = std::array::from_fn(|index| {
            snapshot.resolve(definitions[index]).source.scope
                == orch_settings_contract::provenance::SourceScope::Default
        });
        Ok(Self {
            defaults,
            source: get(keys::FILES_PREVIEW_SOURCE_BYTES)?,
            pixels: u64::try_from(get(keys::FILES_PREVIEW_PIXELS)?).unwrap_or(u64::MAX),
            edge: u32::try_from(get(keys::FILES_PREVIEW_EDGE)?).unwrap_or(u32::MAX),
            bytes: usize::try_from(get(keys::FILES_PREVIEW_BYTES)?).unwrap_or(usize::MAX),
            cache: usize::try_from(get(keys::FILES_PREVIEW_CACHE_BYTES)?).unwrap_or(usize::MAX),
            lines: usize::try_from(get(keys::REFERENCES_PREVIEW_LINES)?)
                .unwrap_or(usize::MAX)
                .saturating_mul(2)
                .saturating_add(1),
            viewer: get(keys::FILES_VIEWER_BYTES)?,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Ready,
    Unavailable,
    TooLarge,
}

/// A preview is one bounded reply, including its static PNG when there is one.
#[derive(Debug, Serialize)]
struct Preview {
    allowance: Option<orch_api_contract::workspace::PreviewAllowance>,
    viewer_allowance: orch_api_contract::workspace::PreviewAllowance,
    facts: Option<orch_api_contract::workspace::FileSourceFacts>,
    state: State,
    hash: Option<Hash>,
    size: i64,
    media_type: String,
    is_text: bool,
    image: Option<String>,
    text: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration_seconds: Option<f64>,
    entry_count: Option<i64>,
    viewer_bytes: i64,
}

fn reply(bytes: Bytes) -> Response {
    let mut response = (StatusCode::OK, bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    // Every display rechecks the owner's authority, even when the derived bytes are cached.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response
}

/// Returns a preview after the caller has checked the containing object and exact content hash.
pub async fn serve(ctx: &Ctx, hash: &Hash, name: &str, media_type: &str) -> ApiResult<Response> {
    let limits = Limits::read(ctx).await?;
    let blobs = &ctx.app.blobs;
    // Checking existence precedes the cache: collected originals must not leave readable previews.
    let size = blobs
        .store()
        .size(hash)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    Ok(reply(
        derived(
            blobs.clone(),
            *hash,
            size,
            name.to_owned(),
            media_type.to_owned(),
            limits,
        )
        .await?,
    ))
}

async fn derived(
    blobs: Blobs,
    hash: Hash,
    size: i64,
    name: String,
    media_type: String,
    limits: Limits,
) -> ApiResult<Bytes> {
    let cache = blobs.previews().lock_owned().await;
    derived_with_cache(blobs, cache, hash, size, name, media_type, limits).await
}

/// Uses the same decoder/cache while original transit bytes live only in an ephemeral memory store.
pub async fn serve_bytes(
    ctx: &Ctx,
    bytes: impl Future<Output = ApiResult<Bytes>>,
    name: &str,
    media_type: &str,
) -> ApiResult<Response> {
    let limits = Limits::read(ctx).await?;
    // Queue before collecting an original as well as before decoding it. Readers cannot pile up
    // whole source files behind the shared decoder while their relay slots are already released.
    let cache = ctx.app.blobs.previews().lock_owned().await;
    let bytes = bytes.await?;
    let size = i64::try_from(bytes.len()).map_err(ApiError::internal)?;
    if size > limits.source {
        return unavailable(
            ctx,
            size,
            media_type,
            crate::media::is_text_by_name(name),
            true,
        )
        .await;
    }
    let hash = Hash::of(&bytes);
    let ephemeral = Blobs::in_memory();
    let _publication = ephemeral
        .put_bytes(bytes)
        .await
        .map_err(ApiError::internal)?;
    let derived = derived_with_cache(
        ephemeral,
        cache,
        hash,
        size,
        name.to_owned(),
        media_type.to_owned(),
        limits,
    )
    .await?;
    Ok(reply(derived))
}

async fn derived_with_cache(
    blobs: Blobs,
    mut cache: tokio::sync::OwnedMutexGuard<Cache>,
    hash: Hash,
    size: i64,
    name: String,
    media_type: String,
    limits: Limits,
) -> ApiResult<Bytes> {
    let key = format!(
        "{hash}:{media_type}:{}:{}:{}:{}:{}:{}:{:?}",
        limits.source,
        limits.pixels,
        limits.edge,
        limits.bytes,
        limits.lines,
        limits.viewer,
        limits.defaults
    );
    // One decode at a time per store also bounds aggregate pixel allocation and coalesces same-hash requests.
    cache.trim(limits.cache);
    if let Some(bytes) = cache.take(&key) {
        return Ok(bytes);
    }
    // A reader leaving must not detach an unbounded second decode or discard the work's cache entry.
    tokio::spawn(async move {
        let preview = make(&blobs, hash, size, &name, &media_type, limits).await?;
        let bytes = Bytes::from(serde_json::to_vec(&preview).map_err(ApiError::internal)?);
        cache.put(key, bytes.clone(), limits.cache);
        Ok(bytes)
    })
    .await
    .map_err(ApiError::internal)?
}

async fn prefix(blobs: &Blobs, hash: &Hash, size: i64, limit: usize) -> ApiResult<Bytes> {
    if size == 0 {
        return Ok(Bytes::new());
    }
    let end = size
        .min(i64::try_from(limit).unwrap_or(i64::MAX))
        .saturating_sub(1);
    let read = blobs
        .read(hash, Some(ByteRange::Between { start: 0, end }))
        .await
        .map_err(ApiError::internal)?;
    read_all(read.body).await.map_err(ApiError::internal)
}

async fn make(
    blobs: &Blobs,
    hash: Hash,
    size: i64,
    name: &str,
    media: &str,
    limits: Limits,
) -> ApiResult<Preview> {
    let mut preview = Preview {
        allowance: None,
        viewer_allowance: limits.allowance(
            orch_api_contract::workspace::PreviewAllowanceKind::ViewerBytes,
            None,
        ),
        facts: None,
        state: State::Unavailable,
        hash: Some(hash),
        size,
        media_type: media.to_owned(),
        is_text: false,
        image: None,
        text: None,
        width: None,
        height: None,
        duration_seconds: None,
        entry_count: None,
        viewer_bytes: limits.viewer,
    };
    if media.starts_with("image/") {
        if size > limits.source {
            preview.state = State::TooLarge;
            preview.allowance = Some(limits.allowance(
                orch_api_contract::workspace::PreviewAllowanceKind::DecodedSourceBytes,
                u64::try_from(size).ok(),
            ));
            return Ok(preview);
        }
        let bytes = blobs
            .read_whole(&hash, limits.source)
            .await
            .map_err(ApiError::internal)?;
        let vector = media == "image/svg+xml";
        return tokio::task::spawn_blocking(move || {
            match if vector {
                svg(&bytes, limits)
            } else {
                raster(&bytes, limits)
            } {
                Ok((png, width, height)) => {
                    preview.state = State::Ready;
                    preview.image = Some(format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(png)
                    ));
                    preview.width = Some(width);
                    preview.height = Some(height);
                }
                Err(refusal) => {
                    preview.allowance = match refusal {
                        Refusal::Pixels(actual) => Some(limits.allowance(
                            orch_api_contract::workspace::PreviewAllowanceKind::Pixels,
                            actual,
                        )),
                        Refusal::EncodedBytes(actual) => Some(limits.allowance(
                            orch_api_contract::workspace::PreviewAllowanceKind::EncodedPreviewBytes,
                            Some(actual),
                        )),
                        Refusal::Unavailable => None,
                    };
                    preview.state = if preview.allowance.is_some() {
                        State::TooLarge
                    } else {
                        State::Unavailable
                    };
                }
            }
            preview
        })
        .await
        .map_err(ApiError::internal);
    }
    let head = prefix(
        blobs,
        &hash,
        size,
        limits.bytes.min(crate::media::SNIFF_BYTES),
    )
    .await?;
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        preview.entry_count = crate::archives::preview_count(
            blobs,
            orch_blob_contract::range::BlobMeta { hash, size },
        )
        .await
        .ok();
        if preview.entry_count.is_some() {
            preview.state = State::Ready;
        }
    }
    preview.is_text = crate::media::is_text_by_name(name)
        || (media == crate::media::UNKNOWN && crate::media::is_text(&head));
    if preview.is_text {
        // An excerpt stays inert text in every card, including HTML and Markdown.
        let characters = orch_api_contract::documents::REFERENCE_PREVIEW_TEXT_MAX_CHARS as usize;
        let text = prefix(blobs, &hash, size, limits.bytes.min(characters * 4)).await?;
        preview.text = Some(excerpt(&text, limits.lines, characters));
        preview.state = State::Ready;
    } else if media == "audio/wav"
        && head.len() >= WAV_HEADER_BYTES
        && head.starts_with(b"RIFF")
        && head.get(8..12) == Some(b"WAVE")
    {
        let rate = u32::from_le_bytes(head[28..32].try_into().unwrap_or_default());
        let data = u32::from_le_bytes(head[40..44].try_into().unwrap_or_default());
        if rate > 0 && head.get(36..40) == Some(b"data") {
            preview.duration_seconds = Some(f64::from(data) / f64::from(rate));
            preview.state = State::Ready;
        }
    }
    Ok(preview)
}

fn excerpt(bytes: &[u8], lines: usize, characters: usize) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(characters)
        .collect::<String>()
        .lines()
        .take(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The policy accepted before an owner-specific preview starts work.
#[derive(Debug, Clone, Copy)]
pub struct Policy(Limits);

impl Policy {
    /// Original bytes admitted before asking a machine for content.
    #[must_use]
    pub fn source_bytes(self) -> i64 {
        self.0.source
    }
}

/// Resolve the policy once; native fetch and any refusal use this same receipt.
pub async fn policy(ctx: &Ctx) -> ApiResult<Policy> {
    Ok(Policy(Limits::read(ctx).await?))
}

/// Owner-specific source facts are added outside the content-addressed derived cache.
pub async fn serve_source(
    ctx: &Ctx,
    hash: &Hash,
    name: &str,
    media_type: &str,
    policy: Policy,
    facts: Option<orch_api_contract::workspace::FileSourceFacts>,
) -> ApiResult<Response> {
    let size = ctx
        .app
        .blobs
        .store()
        .size(hash)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let bytes = derived(
        ctx.app.blobs.clone(),
        *hash,
        size,
        name.to_owned(),
        media_type.to_owned(),
        policy.0,
    )
    .await?;
    let mut preview: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(ApiError::internal)?;
    preview["facts"] = serde_json::to_value(facts).map_err(ApiError::internal)?;
    Ok(reply(Bytes::from(
        serde_json::to_vec(&preview).map_err(ApiError::internal)?,
    )))
}

/// A described original that cannot be decoded or brought cheaply keeps its accepted policy.
pub async fn unavailable(
    _ctx: &Ctx,
    size: i64,
    media_type: &str,
    is_text: bool,
    too_large: bool,
    policy: Policy,
    facts: Option<orch_api_contract::workspace::FileSourceFacts>,
) -> ApiResult<Response> {
    let limits = policy.0;
    let preview = Preview {
        allowance: too_large.then(|| {
            limits.allowance(
                orch_api_contract::workspace::PreviewAllowanceKind::DecodedSourceBytes,
                u64::try_from(size).ok(),
            )
        }),
        viewer_allowance: limits.allowance(
            orch_api_contract::workspace::PreviewAllowanceKind::ViewerBytes,
            None,
        ),
        facts,
        state: if too_large {
            State::TooLarge
        } else {
            State::Unavailable
        },
        hash: None,
        size,
        media_type: media_type.to_owned(),
        is_text,
        image: None,
        text: None,
        width: None,
        height: None,
        duration_seconds: None,
        entry_count: None,
        viewer_bytes: limits.viewer,
    };
    Ok(reply(Bytes::from(
        serde_json::to_vec(&preview).map_err(ApiError::internal)?,
    )))
}

impl Limits {
    fn allowance(
        self,
        kind: orch_api_contract::workspace::PreviewAllowanceKind,
        actual_if_known: Option<u64>,
    ) -> orch_api_contract::workspace::PreviewAllowance {
        use orch_api_contract::workspace::PreviewAllowanceKind;
        let (limit, index) = match kind {
            PreviewAllowanceKind::DecodedSourceBytes => {
                (u64::try_from(self.source).unwrap_or_default(), 0)
            }
            PreviewAllowanceKind::Pixels => (self.pixels, 1),
            PreviewAllowanceKind::EncodedPreviewBytes => (self.bytes as u64, 3),
            PreviewAllowanceKind::ViewerBytes => {
                (u64::try_from(self.viewer).unwrap_or_default(), 6)
            }
        };
        orch_api_contract::workspace::PreviewAllowance {
            kind,
            limit,
            actual_if_known,
            source: orch_api_contract::settings::SettingSource {
                scope: if self.defaults[index] {
                    orch_api_contract::settings::SettingScope::Default
                } else {
                    orch_api_contract::settings::SettingScope::System
                },
                node: None,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Unavailable,
    Pixels(Option<u64>),
    EncodedBytes(u64),
}

/// A PCM WAV's fixed header includes the byte rate and data length.
const WAV_HEADER_BYTES: usize = 44;

/// Floating-point RGBA has sixteen bytes per pixel; the decoder also enforces dimensions.
const DECODE_BYTES_PER_PIXEL: u64 = 16;

fn raster(bytes: &[u8], limits: Limits) -> Result<(Vec<u8>, u32, u32), Refusal> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Refusal::Unavailable)?;
    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(u32::try_from(limits.pixels).unwrap_or(u32::MAX));
    decode_limits.max_image_height = decode_limits.max_image_width;
    decode_limits.max_alloc = Some(limits.pixels.saturating_mul(DECODE_BYTES_PER_PIXEL));
    reader.limits(decode_limits);
    let mut decoder = reader.into_decoder().map_err(|error| {
        if matches!(error, image::ImageError::Limits(_)) {
            Refusal::Pixels(None)
        } else {
            Refusal::Unavailable
        }
    })?;
    let (width, height) = decoder.dimensions();
    if u64::from(width).saturating_mul(u64::from(height)) > limits.pixels {
        return Err(Refusal::Pixels(Some(
            u64::from(width).saturating_mul(u64::from(height)),
        )));
    }
    let orientation = decoder.orientation().map_err(|_| Refusal::Unavailable)?;
    // GIF/WebP decode their first frame only; previews never animate.
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| {
        if matches!(error, image::ImageError::Limits(_)) {
            Refusal::Pixels(None)
        } else {
            Refusal::Unavailable
        }
    })?;
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    let edge = limits.edge.min(width.max(height));
    let png = encode(image.thumbnail(edge, edge), limits.bytes)?;
    Ok((png, width, height))
}

fn encode(mut image: DynamicImage, maximum: usize) -> Result<Vec<u8>, Refusal> {
    loop {
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|_| Refusal::Unavailable)?;
        if png.get_ref().len() <= maximum {
            return Ok(png.into_inner());
        }
        if image.width() <= 1 && image.height() <= 1 {
            return Err(Refusal::EncodedBytes(png.get_ref().len() as u64));
        }
        // Halving bounds the retries logarithmically when the operator chooses a tiny byte budget.
        image = image.thumbnail((image.width() / 2).max(1), (image.height() / 2).max(1));
    }
}

fn svg(bytes: &[u8], limits: Limits) -> Result<(Vec<u8>, u32, u32), Refusal> {
    let embedded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen_embedded = std::sync::Arc::clone(&embedded);
    let options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(move |_, _, _| {
                seen_embedded.store(true, std::sync::atomic::Ordering::Relaxed);
                None
            }),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    // No network, filesystem images, fonts, scripts or embedded raster decoders are consulted.
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|_| Refusal::Unavailable)?;
    // Embedded raster decoders need their own pixel admission. Until supported, report an unavailable preview
    // instead of returning a misleading blank thumbnail or letting the browser decode an unchecked original.
    if embedded.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(Refusal::Unavailable);
    }
    let original = tree.size().to_int_size();
    if u64::from(original.width()).saturating_mul(u64::from(original.height())) > limits.pixels {
        return Err(Refusal::Pixels(Some(
            u64::from(original.width()).saturating_mul(u64::from(original.height())),
        )));
    }
    let edge = limits.edge.min(original.width().max(original.height()));
    let fitted = original
        .scale_to(resvg::tiny_skia::IntSize::from_wh(edge, edge).ok_or(Refusal::Unavailable)?);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(fitted.width(), fitted.height())
        .ok_or(Refusal::Unavailable)?;
    let scale = fitted.width() as f32 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    let image = image::RgbaImage::from_raw(fitted.width(), fitted.height(), rgba)
        .ok_or(Refusal::Unavailable)?;
    Ok((
        encode(DynamicImage::ImageRgba8(image), limits.bytes)?,
        original.width(),
        original.height(),
    ))
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_blob_contract::store::BlobError;

    use orch_blob_contract::store::BlobRead;

    use orch_blob_contract::store::BlobStore;

    use orch_blob_contract::store::ByteStream;

    use orch_blob_contract::store::Stored;

    use std::future::Future;

    use std::sync::atomic::AtomicUsize;

    use std::sync::atomic::Ordering;
    struct HeldStore {
        inner: crate::blobs::MemoryBlobs,
        reads: AtomicUsize,
        began: tokio::sync::Notify,
        gate: tokio::sync::Semaphore,
    }

    #[async_trait::async_trait]
    impl BlobStore for HeldStore {
        fn lifetimes(&self) -> &orch_blob_contract::lifetime::Lifetimes {
            self.inner.lifetimes()
        }

        async fn put_held(
            &self,
            hash: &Hash,
            size: Option<i64>,
            bytes: ByteStream,
            publication: &orch_blob_contract::lifetime::Publication,
        ) -> Result<Stored, BlobError> {
            publication.check(self.lifetimes(), hash)?;
            self.inner.put_held(hash, size, bytes, publication).await
        }
        async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.began.notify_one();
            // Closing the fixture's gate releases both current and future readers.
            let _ = self.gate.acquire().await;
            self.inner.get(hash, range).await
        }
        async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
            self.inner.exists(hash).await
        }
        async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
            self.inner.size(hash).await
        }
        async fn delete_held(
            &self,
            hash: &Hash,
            retirement: &orch_blob_contract::lifetime::Retirement,
        ) -> Result<bool, BlobError> {
            retirement.check(self.lifetimes(), hash)?;
            self.inner.delete_held(hash, retirement).await
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn __api_run_a_refusal_keeps_its_particular_allowance_when_the_policy_later_changes()
    {
        let mut original = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(10, 10)
            .write_to(&mut original, ImageFormat::Png)
            .unwrap();
        let bytes = original.into_inner();
        let store = Blobs::in_memory();
        let meta = store.put_bytes(Bytes::from(bytes)).await.unwrap();
        let first = make(
            &store,
            meta.hash,
            meta.size,
            "image.png",
            "image/png",
            Limits {
                source: 1,
                ..limits()
            },
        )
        .await
        .unwrap();
        let pixels = make(
            &store,
            meta.hash,
            meta.size,
            "image.png",
            "image/png",
            Limits {
                pixels: 99,
                ..limits()
            },
        )
        .await
        .unwrap();
        let encoded = make(
            &store,
            meta.hash,
            meta.size,
            "image.png",
            "image/png",
            Limits {
                bytes: 1,
                ..limits()
            },
        )
        .await
        .unwrap();
        let ready = make(
            &store,
            meta.hash,
            meta.size,
            "image.png",
            "image/png",
            limits(),
        )
        .await
        .unwrap();
        assert!(matches!(ready.state, State::Ready));
        assert_eq!(
            first.allowance.as_ref().unwrap().kind,
            orch_api_contract::workspace::PreviewAllowanceKind::DecodedSourceBytes
        );
        assert_eq!(first.allowance.unwrap().limit, 1);
        assert_eq!(
            pixels.allowance.as_ref().unwrap().kind,
            orch_api_contract::workspace::PreviewAllowanceKind::Pixels
        );
        assert_eq!(pixels.allowance.unwrap().actual_if_known, Some(100));
        assert_eq!(
            encoded.allowance.unwrap().kind,
            orch_api_contract::workspace::PreviewAllowanceKind::EncodedPreviewBytes
        );
    }

    fn limits() -> Limits {
        Limits {
            defaults: [true; 7],
            source: 1_048_576,
            pixels: 1_000_000,
            edge: 64,
            bytes: 16_384,
            cache: 1_048_576,
            lines: 5,
            viewer: 1_048_576,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn text_cards_share_the_reference_card_bound_even_for_one_very_long_line() {
        let characters = orch_api_contract::documents::REFERENCE_PREVIEW_TEXT_MAX_CHARS as usize;
        assert_eq!(
            excerpt("字".repeat(10000).as_bytes(), 5, characters)
                .chars()
                .count(),
            characters
        );
        assert_eq!(excerpt(b"one\ntwo\nthree\nfour", 2, characters), "one\ntwo");
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_text_cards_share_the_reference_card_bound_even_for_one_very_long_line() {
        self::text_cards_share_the_reference_card_bound_even_for_one_very_long_line()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn active_original_types_are_inert_downloads_from_the_shared_file_server() {
        use orch_blob_contract::range::{BlobMeta, ResolvedRange};
        for media in [
            "text/html",
            "Text/HTML; charset=utf-8",
            "image/svg+xml",
            "application/xml",
            "multipart/mixed",
        ] {
            let hash = Hash::of(b"");
            let read = BlobRead {
                meta: BlobMeta { hash, size: 0 },
                range: ResolvedRange { start: 0, len: 0 },
                body: Box::pin(futures::stream::empty()),
            };
            let response = crate::routes::workspace::files::serve(
                read,
                false,
                &http::HeaderMap::new(),
                &hash,
                "file",
                media,
                false,
            );
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/octet-stream"
            );
            assert!(
                response.headers()[header::CONTENT_DISPOSITION]
                    .to_str()
                    .unwrap()
                    .starts_with("attachment;")
            );
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert!(
                response.headers()[header::CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .contains("sandbox")
            );
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-cache"
            );
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_active_original_types_are_inert_downloads_from_the_shared_file_server() {
        self::active_original_types_are_inert_downloads_from_the_shared_file_server()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn previews_are_bounded_static_images_and_keep_transparency() {
        let original = DynamicImage::ImageRgba8(image::RgbaImage::new(800, 600));
        let mut bytes = Cursor::new(Vec::new());
        original.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let (png, width, height) = raster(bytes.get_ref(), limits()).unwrap();
        assert_eq!((width, height), (800, 600));
        assert!(png.len() <= limits().bytes);
        let thumb = image::load_from_memory(&png).unwrap();
        assert!(thumb.width() <= 64 && thumb.height() <= 64);
        assert_eq!(thumb.to_rgba8().get_pixel(0, 0).0[3], 0);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_previews_are_bounded_static_images_and_keep_transparency() {
        self::previews_are_bounded_static_images_and_keep_transparency()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn an_image_over_the_pixel_budget_is_refused_before_decode() {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(100, 100)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        assert_eq!(
            raster(
                bytes.get_ref(),
                Limits {
                    pixels: 9999,
                    ..limits()
                }
            ),
            Err(Refusal::Pixels(Some(10000)))
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_an_image_over_the_pixel_budget_is_refused_before_decode() {
        self::an_image_over_the_pixel_budget_is_refused_before_decode()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn the_static_preview_applies_the_original_exif_orientation() {
        let mut original = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(2, 3)
            .write_to(&mut original, ImageFormat::Jpeg)
            .unwrap();
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
        jpeg.extend_from_slice(exif);
        jpeg.extend_from_slice(&original.get_ref()[2..]);
        let (png, width, height) = raster(&jpeg, limits()).unwrap();
        assert_eq!((width, height), (3, 2));
        let thumbnail = image::load_from_memory(&png).unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (3, 2));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_static_preview_applies_the_original_exif_orientation() {
        self::the_static_preview_applies_the_original_exif_orientation()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn an_animated_image_produces_only_its_first_frame_as_a_png() {
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            encoder
                .encode_frames([
                    image::Frame::new(image::RgbaImage::from_pixel(
                        2,
                        2,
                        image::Rgba([255, 0, 0, 255]),
                    )),
                    image::Frame::new(image::RgbaImage::from_pixel(
                        2,
                        2,
                        image::Rgba([0, 0, 255, 255]),
                    )),
                ])
                .unwrap();
        }
        let (png, _, _) = raster(&gif, limits()).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(
            image::load_from_memory(&png)
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)
                .0,
            [255, 0, 0, 255]
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_an_animated_image_produces_only_its_first_frame_as_a_png() {
        self::an_animated_image_produces_only_its_first_frame_as_a_png()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn svg_is_rasterized_without_resolving_external_images_or_running_scripts() {
        let vector = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80"><script>throw 1</script><image href="file:///etc/passwd"/><rect width="100" height="80" fill="red"/></svg>"#;
        let (png, width, height) = svg(vector, limits()).unwrap();
        assert_eq!((width, height), (100, 80));
        assert!(png.starts_with(b"\x89PNG"));
        assert!(!String::from_utf8_lossy(&png).contains("passwd"));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_svg_is_rasterized_without_resolving_external_images_or_running_scripts() {
        self::svg_is_rasterized_without_resolving_external_images_or_running_scripts()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn the_cache_is_byte_bounded_and_discards_the_least_recently_used_preview() {
        let mut cache = Cache::default();
        cache.put("a".into(), Bytes::from_static(b"1234"), 8);
        cache.put("b".into(), Bytes::from_static(b"5678"), 8);
        assert!(cache.take("a").is_some());
        cache.put("c".into(), Bytes::from_static(b"9012"), 8);
        assert!(cache.take("b").is_none());
        assert!(cache.take("a").is_some());
        cache.trim(0);
        assert_eq!(cache.bytes, 0);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_cache_is_byte_bounded_and_discards_the_least_recently_used_preview() {
        self::the_cache_is_byte_bounded_and_discards_the_least_recently_used_preview()
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn a_reader_leaving_does_not_repeat_the_same_original_read_or_lose_its_cached_preview() {
        let store = Arc::new(HeldStore {
            inner: crate::blobs::MemoryBlobs::default(),
            reads: AtomicUsize::new(0),
            began: tokio::sync::Notify::new(),
            gate: tokio::sync::Semaphore::new(0),
        });
        let blobs = Blobs::new(store.clone(), std::path::Path::new("."));
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(32, 16)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let meta = blobs
            .put_bytes(Bytes::from(png.into_inner()))
            .await
            .unwrap();
        let first = tokio::spawn(derived(
            blobs.clone(),
            meta.hash,
            meta.size,
            "image.png".into(),
            "image/png".into(),
            limits(),
        ));
        store.began.notified().await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let second = tokio::spawn(derived(
            blobs,
            meta.hash,
            meta.size,
            "image.png".into(),
            "image/png".into(),
            limits(),
        ));
        store.gate.close();
        let preview: serde_json::Value =
            serde_json::from_slice(&second.await.unwrap().unwrap()).unwrap();
        assert_eq!(preview["state"], "ready");
        assert_eq!(
            store.reads.load(Ordering::SeqCst),
            1,
            "a visible retry joins the one decode that began"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_reader_leaving_does_not_repeat_the_same_original_read_or_lose_its_cached_preview()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(self::a_reader_leaving_does_not_repeat_the_same_original_read_or_lose_its_cached_preview())
    }
}
