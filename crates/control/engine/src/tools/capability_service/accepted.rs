//! Prepared provider input and retained accepted output are data, never replacement source authority.
use super::*;
use futures::StreamExt;
use orch_blob_contract::store::{Verifier, stream_of};
use orch_provider_contract::capabilities::Binding;
use std::sync::{Arc, Mutex};

/// An opaque reference to generated output retained by the actual native operation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityResultReceipt {
    owner: source::Source,
    command: Id,
    capability: String,
    input_hash: Hash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    material_hash: Option<Hash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    helper: Option<delegation::ParentLink>,
}
impl std::fmt::Debug for CapabilityResultReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CapabilityResultReceipt([private])")
    }
}
/// Complete accepted data, suitable for a consumer's own retained-result journal before file delivery.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedCapabilityResult {
    receipt: CapabilityResultReceipt,
    input: Json,
    binding: Binding,
    raw: Json,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    native_images: Vec<NativeImage>,
}

/// Exact retained native image evidence, without another URL or filesystem transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeImage {
    /// The exact blob retained by the native image item.
    pub blob: Hash,
    /// Its verified retained byte size.
    pub size: i64,
    /// Its reported media type.
    pub media_type: String,
    /// The native generation item identity.
    pub native_ref: String,
    /// The actual revised prompt, when reported.
    pub revised_prompt: Option<String>,
}
impl std::fmt::Debug for AcceptedCapabilityResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcceptedCapabilityResult([private])")
    }
}
impl AcceptedCapabilityResult {
    /// Actual accepted identity; it does not authorize another paid call or a file write.
    pub fn receipt(&self) -> &CapabilityResultReceipt {
        &self.receipt
    }
    /// The provider-neutral input, with consumer-local paths kept separate.
    pub fn input(&self) -> &Json {
        &self.input
    }
    /// The exact installed result codec retained with that accepted response.
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    /// Actual API response or the model binding's declared native envelope, for its owning materializer.
    pub fn raw_result(&self) -> &Json {
        &self.raw
    }
    /// Native bytes already retained through the ordinary item/blob path.
    pub fn native_images(&self) -> &[NativeImage] {
        &self.native_images
    }

    /// The same common projection; image consumers can instead use their declared byte decoder.
    pub fn project(&self, limit: usize) -> Result<Json, EngineError> {
        orch_provider_contract::capabilities::project_result(
            &self.binding,
            &self.input,
            &self.raw,
            limit,
        )
        .map_err(EngineError::unreadable)
    }
}
struct Prepared {
    owner: source::Source,
    capability: String,
    input: Json,
    images: Vec<orch_model_contract::generation::GenerationPart>,
    accepted: Arc<Mutex<Option<AcceptedCapabilityResult>>>,
}
tokio::task_local! { static PREPARED: Prepared; }
const SLOT: &str = "accepted_service_result";

pub(super) fn is_prepared() -> bool {
    PREPARED.try_with(|_| true).unwrap_or(false)
}

pub(super) fn input(call: &CallContext, capability: &str) -> Result<Json, EngineError> {
    PREPARED
        .try_with(|prepared| {
            if prepared.owner != source::Source::captured()? || prepared.capability != capability {
                return Err(EngineError::ToolCallIdentity);
            }
            Ok(prepared.input.clone())
        })
        .unwrap_or_else(|_| Ok(call.arguments.clone()))
}
pub(super) fn images() -> Vec<orch_model_contract::generation::GenerationPart> {
    PREPARED
        .try_with(|prepared| prepared.images.clone())
        .unwrap_or_default()
}
fn material_hash(
    images: &[orch_model_contract::generation::GenerationPart],
) -> Result<Option<Hash>, EngineError> {
    if images.is_empty() {
        Ok(None)
    } else {
        source::hash_json(serde_json::to_value(images)?).map(Some)
    }
}

pub(super) fn capture(
    call: &CallContext,
    capability: &str,
    command: Id,
    input: &Json,
    binding: &Binding,
    raw: &Json,
) -> Result<bool, EngineError> {
    let _ = call;
    PREPARED
        .try_with(|prepared| {
            if prepared.owner != source::Source::captured()?
                || prepared.capability != capability
                || &prepared.input != input
            {
                return Err(EngineError::ToolCallIdentity);
            }
            let value = AcceptedCapabilityResult {
                receipt: CapabilityResultReceipt {
                    owner: prepared.owner,
                    command,
                    capability: capability.into(),
                    input_hash: source::hash_json(input.clone())?,
                    material_hash: material_hash(&prepared.images)?,
                    helper: None,
                },
                input: input.clone(),
                binding: binding.clone(),
                raw: raw.clone(),
                native_images: Vec::new(),
            };
            *prepared.accepted.lock().map_err(|_| {
                EngineError::unreadable("the accepted service result is unavailable")
            })? = Some(value);
            Ok(true)
        })
        .unwrap_or(Ok(false))
}
pub(super) fn capture_native(
    call: &CallContext,
    capability: &str,
    command: Id,
    helper: delegation::ParentLink,
    input: &Json,
    images: Vec<NativeImage>,
    details: Json,
) -> Result<AcceptedCapabilityResult, EngineError> {
    if images.is_empty()
        || images.iter().any(|image| {
            image.size <= 0
                || !image.media_type.starts_with("image/")
                || image.native_ref.is_empty()
        })
    {
        return Err(EngineError::ToolCallIdentity);
    }
    PREPARED
        .try_with(|prepared| {
            if prepared.owner != source::Source::captured()?
                || prepared.capability != capability
                || &prepared.input != input
            {
                return Err(EngineError::ToolCallIdentity);
            }
            let result = AcceptedCapabilityResult {
                receipt: CapabilityResultReceipt {
                    owner: prepared.owner,
                    command,
                    capability: capability.into(),
                    input_hash: source::hash_json(input.clone())?,
                    material_hash: material_hash(&prepared.images)?,
                    helper: Some(helper),
                },
                input: input.clone(),
                binding: Binding::default(),
                raw: details,
                native_images: images,
            };
            *prepared.accepted.lock().map_err(|_| {
                EngineError::unreadable("the accepted native result is unavailable")
            })? = Some(result.clone());
            let _ = call;
            Ok(result)
        })
        .map_err(|_| EngineError::StaleToolSource)?
}

impl Engine {
    pub(super) async fn retain_native_capability_result(
        &self,
        call: &CallContext,
        accepted: &AcceptedCapabilityResult,
    ) -> Result<(), EngineError> {
        if accepted.receipt.owner != source::Source::captured()?
            || accepted.receipt.helper.is_none()
            || accepted.native_images.is_empty()
        {
            return Err(EngineError::ToolCallIdentity);
        }
        let value = serde_json::to_value(accepted)?;
        let size =
            i64::try_from(serde_json::to_vec(&value)?.len()).map_err(EngineError::unreadable)?;
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        let mut hashes: Vec<Hash> = accepted
            .native_images
            .iter()
            .map(|image| image.blob)
            .collect();
        hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        hashes.dedup();
        orch_db::queries::uploads::lock_blobs(&mut tx, &hashes).await?;
        let present = orch_db::queries::storage::blobs_among(&mut tx, &hashes).await?;
        let pins: Vec<Vec<u8>> = present
            .iter()
            .map(|hash| hash.as_bytes().to_vec())
            .collect();
        let wrote=sqlx::query("INSERT INTO thread_tool_service_results(answer_id,command_id,capability,input_hash,result_size,result_images,inline_native) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(answer_id) DO NOTHING")
            .bind(accepted.receipt.owner.answer()).bind(accepted.receipt.command).bind(&accepted.receipt.capability).bind(accepted.receipt.input_hash.as_bytes().as_slice())
            .bind(size).bind(pins).bind(value).execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected();
        if wrote == 1 {
            orch_db::queries::storage::pin_blobs(&mut tx, &present).await?;
        }
        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,ARRAY[$2::text],$3) WHERE answer_id=$1 AND NOT(plan ? $2)")
            .bind(accepted.receipt.owner.answer()).bind(SLOT).bind(serde_json::to_value(&accepted.receipt)?).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Run the same admitted capability path with consumer-prepared neutral input and retain its accepted output.
    /// This does not authorize local paths or perform file materialization.
    pub async fn execute_prepared_capability_service(
        &self,
        call: &CallContext,
        capability: &str,
        input: Json,
    ) -> Result<AcceptedCapabilityResult, EngineError> {
        self.execute_materialized_capability_service(call, capability, input, Vec::new())
            .await
    }

    /// Supply actual source-bound image blobs separately from neutral input. Only ImageRef is accepted.
    /// API multipart fields refer to these refs by their exact name; model inputs retain their order.
    pub async fn execute_materialized_capability_service(
        &self,
        call: &CallContext,
        capability: &str,
        input: Json,
        images: Vec<orch_model_contract::generation::GenerationPart>,
    ) -> Result<AcceptedCapabilityResult, EngineError> {
        if images.iter().any(|image| !matches!(image,
            orch_model_contract::generation::GenerationPart::ImageRef { size, media_type, name, .. }
                if *size > 0 && media_type.starts_with("image/") && !name.is_empty()
        )) {
            return Err(EngineError::unreadable("prepared image inputs require actual source-bound image references"));
        }
        let expected_material = material_hash(&images)?;
        let owner = source::Source::captured()?;
        let saved: Option<Json> = {
            let mut conn = self.db().acquire().await?;
            source::check_call(&mut conn, call).await?;
            sqlx::query_scalar("SELECT plan->$2 FROM thread_tool_calls WHERE answer_id=$1")
                .bind(owner.answer())
                .bind(SLOT)
                .fetch_one(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
        };
        if let Some(saved) = saved {
            let receipt: CapabilityResultReceipt = serde_json::from_value(saved)?;
            if receipt.capability != capability
                || receipt.input_hash != source::hash_json(input)?
                || receipt.material_hash != expected_material
            {
                return Err(EngineError::ToolCallIdentity);
            }
            return self.read_accepted_capability_result(call, &receipt).await;
        }
        let sink = Arc::new(Mutex::new(None));
        let attempted = PREPARED
            .scope(
                Prepared {
                    owner,
                    capability: capability.into(),
                    input,
                    images,
                    accepted: sink.clone(),
                },
                self.execute_capability_service(call, capability),
            )
            .await;
        let accepted = sink
            .lock()
            .map_err(|_| EngineError::unreadable("the accepted service result is unavailable"))?
            .take();
        let Some(accepted) = accepted else {
            return Err(attempted.err().unwrap_or_else(|| {
                EngineError::unreadable("the capability returned no accepted native result")
            }));
        };
        if accepted.receipt.helper.is_some() {
            self.retain_native_capability_result(call, &accepted)
                .await?;
            return self
                .read_accepted_capability_result(call, &accepted.receipt)
                .await;
        }
        // An accepted native answer survives a later projection or file-delivery failure.
        let bytes = serde_json::to_vec(&accepted)?;
        let hash = Hash::of(&bytes);
        let size = i64::try_from(bytes.len()).map_err(EngineError::unreadable)?;
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("the accepted result store is unavailable"))?;
        store
            .put(
                &hash,
                Some(size),
                stream_of(vec![bytes::Bytes::from(bytes)]),
            )
            .await
            .map_err(EngineError::unreadable)?;
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        orch_db::queries::storage::register_blob(&mut tx, &hash, size, self.inner.clock.now())
            .await?;
        let mut images: Vec<Hash> = accepted
            .native_images
            .iter()
            .map(|image| image.blob)
            .collect();
        images.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        images.dedup();
        let raw_images: Vec<Vec<u8>> = images.iter().map(|hash| hash.as_bytes().to_vec()).collect();
        let wrote=sqlx::query("INSERT INTO thread_tool_service_results(answer_id,command_id,capability,input_hash,result_blob,result_size,result_images) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(answer_id) DO NOTHING")
            .bind(owner.answer()).bind(accepted.receipt.command).bind(&accepted.receipt.capability).bind(accepted.receipt.input_hash.as_bytes().as_slice()).bind(hash.as_bytes().as_slice()).bind(size).bind(raw_images).execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected();
        if wrote == 1 {
            images.push(hash);
            orch_db::queries::uploads::lock_blobs(&mut tx, &images).await?;
            let pinned = orch_db::queries::storage::pin_blobs(&mut tx, &images).await?;
            if usize::try_from(pinned).map_err(EngineError::unreadable)? != images.len() {
                return Err(EngineError::StaleToolSource);
            }
        }
        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,ARRAY[$2::text],$3) WHERE answer_id=$1 AND NOT(plan ? $2)")
            .bind(owner.answer()).bind(SLOT).bind(serde_json::to_value(&accepted.receipt)?).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.read_accepted_capability_result(call, &accepted.receipt)
            .await
    }

    /// Read retained output within the same actual native execution. A consumer's independent job needs its own Read source.
    pub async fn read_accepted_capability_result(
        &self,
        call: &CallContext,
        receipt: &CapabilityResultReceipt,
    ) -> Result<AcceptedCapabilityResult, EngineError> {
        let current = source::Source::captured()?;
        if !current.same_execution(receipt.owner) {
            return Err(EngineError::StaleToolSource);
        }
        let (raw, size, inline): (Option<Vec<u8>>, i64, Option<Json>) = {
            let mut conn = self.db().acquire().await?;
            source::check_call(&mut conn, call).await?;
            source::original_callers(
                &mut conn,
                call,
                self.inner.clock.as_ref(),
                self.inner.access.as_ref(),
            )
            .await?;
            sqlx::query_as("SELECT result_blob,result_size,inline_native FROM thread_tool_service_results WHERE answer_id=$1 AND command_id=$2 AND capability=$3 AND input_hash=$4")
                .bind(receipt.owner.answer()).bind(receipt.command).bind(&receipt.capability).bind(receipt.input_hash.as_bytes().as_slice()).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?.ok_or_else(||EngineError::unreadable("the accepted result is no longer retained; it was not generated again"))?
        };
        let accepted = if let Some(inline) = inline {
            if raw.is_some()
                || i64::try_from(serde_json::to_vec(&inline)?.len())
                    .map_err(EngineError::unreadable)?
                    != size
            {
                return Err(EngineError::ToolCallIdentity);
            }
            let accepted: AcceptedCapabilityResult = serde_json::from_value(inline)?;
            if accepted.receipt.helper.is_none() || accepted.native_images.is_empty() {
                return Err(EngineError::ToolCallIdentity);
            }
            accepted
        } else {
            let hash = Hash::from_slice(&raw.ok_or(EngineError::ToolCallIdentity)?)
                .map_err(EngineError::unreadable)?;
            let store = self.blobs().ok_or_else(|| {
                EngineError::unreadable("the accepted result store is unavailable")
            })?;
            let read = store
                .get(&hash, None)
                .await
                .map_err(EngineError::unreadable)?;
            if read.meta.hash != hash || read.meta.size != size {
                return Err(EngineError::ToolCallIdentity);
            }
            let mut bytes = Vec::new();
            let mut verifier = Verifier::new(hash, Some(size));
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
            serde_json::from_slice::<AcceptedCapabilityResult>(&bytes)?
        };
        if accepted.receipt != *receipt {
            return Err(EngineError::ToolCallIdentity);
        }
        let mut conn = self.db().acquire().await?;
        source::check_call(&mut conn, call).await?;
        source::original_callers(
            &mut conn,
            call,
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await?;
        Ok(accepted)
    }

    /// Release only the temporary accepted-output reference after the consumer durably retains/delivers it.
    pub async fn release_accepted_capability_result(
        &self,
        call: &CallContext,
        receipt: &CapabilityResultReceipt,
    ) -> Result<(), EngineError> {
        if !source::Source::captured()?.same_execution(receipt.owner) {
            return Err(EngineError::StaleToolSource);
        }
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        let raw: Option<ResultBlobs> = sqlx::query_as("DELETE FROM thread_tool_service_results WHERE answer_id=$1 AND command_id=$2 AND capability=$3 AND input_hash=$4 RETURNING result_blob,result_images")
            .bind(receipt.owner.answer()).bind(receipt.command).bind(&receipt.capability).bind(receipt.input_hash.as_bytes().as_slice()).fetch_optional(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if let Some((raw, images)) = raw {
            let mut hashes = images
                .iter()
                .map(|raw| Hash::from_slice(raw).map_err(EngineError::unreadable))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(raw) = raw {
                hashes.push(Hash::from_slice(&raw).map_err(EngineError::unreadable)?);
            }
            orch_db::queries::storage::unpin_blobs(&mut tx, &hashes, self.inner.clock.now())
                .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}

/// A stored service result's blob and its images, as the result rows hold them.
type ResultBlobs = (Option<Vec<u8>>, Vec<Vec<u8>>);

pub(crate) async fn reap(engine: &Engine, thread: Option<Id>) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    let raw: Vec<ResultBlobs> = sqlx::query_as("WITH stale AS (
        SELECT r.answer_id FROM thread_tool_service_results r JOIN thread_tool_calls t ON t.answer_id=r.answer_id
        WHERE ($1::uuid IS NULL OR t.thread_id=$1) AND (t.retired OR NOT EXISTS(SELECT 1 FROM threads h WHERE h.node_id=t.thread_id))
        ORDER BY r.answer_id LIMIT $2 FOR UPDATE OF r SKIP LOCKED
    ) DELETE FROM thread_tool_service_results r USING stale s WHERE r.answer_id=s.answer_id RETURNING r.result_blob,r.result_images")
        .bind(thread).bind(engine.inner.config.sweep_batch).fetch_all(&mut *tx).await.map_err(orch_db::DbError::from)?;
    // Each accepted row owns one pin, even if another result retained identical pixels.
    for (blob, images) in raw {
        let mut hashes = images
            .iter()
            .map(|raw| Hash::from_slice(raw).map_err(EngineError::unreadable))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(blob) = blob {
            hashes.push(Hash::from_slice(&blob).map_err(EngineError::unreadable)?);
        }
        orch_db::queries::storage::unpin_blobs(&mut tx, &hashes, engine.inner.clock.now()).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
