//! Shared account-scoped creation replay; only native identities and facts are retained.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use chrono::Duration;

use futures::future::BoxFuture;

use http::StatusCode;

use http::Uri;

use orch_api_contract::common::BASE_PATH;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::IDEMPOTENCY_HEADER;

use orch_api_contract::common::KEY_BASE_PATH;

use orch_db::idempotency as rows;

use orch_db::idempotency::Replay;

use orch_db::idempotency::creation::Capture;

use orch_db::idempotency::creation::Created;

use orch_db::idempotency::creation::Record;

use orch_db::idempotency::creation::Unassigned;

use orch_types::Hash;

use orch_types::Id;

use orch_types::Json;

use orch_types::new_id;

use std::any::Any;

use std::collections::BTreeMap;

use std::collections::HashMap;

use std::sync::Arc;

use std::sync::Mutex;

pub(crate) use claims::Claims;

pub use raw::RawCreation;

pub use raw::RawFormat;

pub(crate) use raw::RawIncoming;

pub use raw::RawInputSpec;

pub(crate) use raw::RawRegistered;
pub type CreationPrincipal<B> = fn(Ctx, B) -> BoxFuture<'static, ApiResult<Option<Id>>>;

pub type CreationReplay<Q, B, T> = fn(Ctx, Q, B, Created) -> BoxFuture<'static, ApiResult<T>>;

/// A native family supplies its actual identity and its own current response Read projector.
/// Neither response bytes nor the original credential belongs to this binding.
pub struct Creation<Q, B, T> {
    pub kind: &'static str,
    pub replay: CreationReplay<Q, B, T>,
    pub identity: fn(&T) -> ApiResult<Created>,
}

/// The native transaction supplies the identity for an empty or identity-free result.
pub struct ObservedCreation<Q, B, T> {
    pub kind: &'static str,
    pub replay: CreationReplay<Q, B, T>,
}

/// A native public creator proves its real account on a completed retry.
/// A first attempt retains that account only in the native creation transaction.
pub struct ProofCreation<Q, B, T> {
    pub kind: &'static str,
    pub principal: CreationPrincipal<B>,
    pub replay: CreationReplay<Q, B, T>,
}

pub struct Registered<Q, B, T> {
    pub kind: &'static str,
    pub principal: Option<CreationPrincipal<B>>,
    pub replay: Option<CreationReplay<Q, B, T>>,
    pub identity: Option<fn(&T) -> ApiResult<Created>>,
    pub raw: Option<RawRegistered<Q>>,
    pub command: Option<commands::CreationCommand>,
    pub resume: Option<Resume>,
}

pub type Resume = fn(&Ctx, &Created) -> ApiResult<bool>;

impl<Q, B, T> From<Creation<Q, B, T>> for Registered<Q, B, T> {
    fn from(value: Creation<Q, B, T>) -> Self {
        Self {
            kind: value.kind,
            principal: None,
            replay: Some(value.replay),
            identity: Some(value.identity),
            raw: None,
            command: None,
            resume: None,
        }
    }
}

impl<Q, B, T> From<ObservedCreation<Q, B, T>> for Registered<Q, B, T> {
    fn from(value: ObservedCreation<Q, B, T>) -> Self {
        Self {
            kind: value.kind,
            principal: None,
            replay: Some(value.replay),
            identity: None,
            raw: None,
            command: None,
            resume: None,
        }
    }
}

impl<Q, B, T> From<ProofCreation<Q, B, T>> for Registered<Q, B, T> {
    fn from(value: ProofCreation<Q, B, T>) -> Self {
        Self {
            kind: value.kind,
            principal: Some(value.principal),
            replay: Some(value.replay),
            identity: None,
            raw: None,
            command: None,
            resume: None,
        }
    }
}

impl<Q, B, T> Registered<Q, B, T> {
    /// The native issuer records this creation on its own queue transaction.
    pub fn command(mut self, command: commands::CreationCommand) -> Self {
        assert!(self.raw.is_none() && self.principal.is_none() && self.resume.is_none());
        self.command = Some(command);
        self
    }

    /// An authenticated native aggregate may resume only its unfinished accepted parts.
    #[must_use]
    pub fn continuing(mut self, resume: Resume) -> Self {
        assert!(
            self.raw.is_none() && self.principal.is_none() && self.command.is_none(),
            "only authenticated JSON aggregates without a single-command root can resume"
        );
        self.resume = Some(resume);
        self
    }
}

#[derive(Default)]
pub struct Creations(Mutex<HashMap<&'static str, Arc<dyn Any + Send + Sync>>>);

impl std::fmt::Debug for Creations {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Creations([native bindings])")
    }
}

impl Creations {
    pub fn insert<Q: 'static, B: 'static, T: 'static>(
        &self,
        name: &'static str,
        value: Registered<Q, B, T>,
    ) {
        // Validate direct registrations too, rather than relying only on a builder's caller.
        let value = match value.resume {
            Some(resume) => value.continuing(resume),
            None => value,
        };
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name, Arc::new(value));
    }
    pub fn get<Q: 'static, B: 'static, T: 'static>(
        &self,
        name: &'static str,
    ) -> ApiResult<Option<Arc<Registered<Q, B, T>>>> {
        self.0
            .lock()
            .map_err(|_| ApiError::internal("creation bindings are poisoned"))?
            .get(name)
            .cloned()
            .map(|value| {
                value.downcast::<Registered<Q, B, T>>().map_err(|_| {
                    ApiError::internal("the creation binding has different wire types")
                })
            })
            .transpose()
    }
}

pub enum Claim {
    New(Pending),
    Saved {
        account: Id,
        record: Record,
        status: StatusCode,
    },
}

pub struct Pending {
    pub capture: Capture,
    method: String,
    path: String,
    lease: claims::Lease,
    resume: Option<Resume>,
}

struct Identified {
    key: String,
    account: Id,
    hash: Hash,
    resume: Option<Resume>,
}

fn mismatch() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictIdempotencyMismatch,
        "the key identifies a different method, path or canonical body",
    )
}

pub fn request_key(ctx: &Ctx) -> ApiResult<Option<String>> {
    let Some(value) = ctx.headers.get(IDEMPOTENCY_HEADER) else {
        return Ok(None);
    };
    if ctx.headers.get_all(IDEMPOTENCY_HEADER).iter().count() != 1 {
        return Err(ApiError::invalid(IDEMPOTENCY_HEADER, "supply one UUID"));
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<Id>().ok())
        .map(|key| Some(key.to_string()))
        .ok_or_else(|| ApiError::invalid(IDEMPOTENCY_HEADER, "must be a UUID"))
}

/// Current private content and one-time-secret acknowledgements are never cached by an intermediary.
pub(crate) fn replay_headers() -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    headers
}

fn canonical(value: Json) -> Json {
    match value {
        Json::Array(values) => Json::Array(values.into_iter().map(canonical).collect()),
        Json::Object(values) => Json::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        other => other,
    }
}

fn body_hash(account: Id, key: &str, bytes: &[u8]) -> ApiResult<Hash> {
    let value = if bytes.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(bytes).map_err(ApiError::internal)?
    };
    // Creation bodies may contain freshly supplied secrets. Keep equality
    // evidence opaque under the existing deployment key rather than exposing
    // a fast, guessable digest of those inputs in the database.
    fingerprint(account, key, "orch.creation.body.v1", canonical(value))
}

fn proof_body_hash(key: &str, bytes: &[u8]) -> ApiResult<Hash> {
    let value = if bytes.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(bytes).map_err(ApiError::internal)?
    };
    let encoded = serde_json::to_vec(&("orch.creation.account-proof.v1", key, canonical(value)))
        .map_err(ApiError::internal)?;
    crate::routes::resources::sealing::installed_master_key()?
        .fingerprint(&encoded)
        .map_err(ApiError::internal)
}

fn fingerprint(account: Id, key: &str, purpose: &str, value: Json) -> ApiResult<Hash> {
    let encoded =
        serde_json::to_vec(&(purpose, account, key, value)).map_err(ApiError::internal)?;
    crate::routes::resources::sealing::installed_master_key()?
        .fingerprint(&encoded)
        .map_err(ApiError::internal)
}

pub enum BodyIdentity<'a> {
    Json(&'a [u8]),
    Raw(&'a RawIncoming),
}

fn target(uri: &Uri) -> String {
    // Browser renewal or another token of the same account keeps the logical route namespace.
    let path = [KEY_BASE_PATH, BASE_PATH]
        .into_iter()
        .find_map(|base| {
            uri.path()
                .strip_prefix(base)
                .filter(|tail| tail.is_empty() || tail.starts_with('/'))
        })
        .unwrap_or(uri.path());
    let path = if path.is_empty() { "/" } else { path };
    uri.query()
        .map_or_else(|| path.to_owned(), |query| format!("{path}?{query}"))
}

/// The one configured replay window also serves native token-secret exceptions.
pub async fn window_since(
    conn: &mut orch_db::Conn,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<chrono::DateTime<chrono::Utc>> {
    let hours = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::API_IDEMPOTENCY_WINDOW,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("idempotency window has no value"))?;
    if hours < 0 {
        return Err(ApiError::internal("idempotency window is negative"));
    }
    Ok(Duration::try_hours(hours)
        .and_then(|span| now.checked_sub_signed(span))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC))
}

impl Claims {
    /// A creation key cannot be reused for another changing request even where
    /// that request is not itself a creation. Native legacy records keep their
    /// own secret-free replay handler.
    pub async fn check_other(&self, ctx: &Ctx, uri: &Uri, bytes: &[u8]) -> ApiResult<()> {
        if ctx.route.method == orch_api_contract::routes::Method::GET
            || ctx.route.idempotency != orch_api_contract::routes::Idempotency::Unspecified
        {
            return Ok(());
        }
        let Some(value) = ctx.headers.get(IDEMPOTENCY_HEADER) else {
            return Ok(());
        };
        let Some(key) = value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<Id>().ok())
        else {
            return Ok(());
        };
        let Ok(user) = ctx.user() else {
            return Ok(());
        };
        let account = user.id();
        let now = ctx.app.clock.now();
        let mut conn = ctx.app.db.acquire().await?;
        let since = window_since(&mut conn, now).await?;
        if let Some(saved) = rows::find(&mut conn, account, &key.to_string(), since).await?
            && saved.response.get("version").and_then(Json::as_u64) == Some(1)
        {
            let record = Record::read(saved.response).map_err(ApiError::internal)?;
            if saved.method != ctx.route.method.as_str()
                || saved.path != target(uri)
                || record.body_hash != body_hash(account, &key.to_string(), bytes)?
            {
                return Err(mismatch());
            }
        }
        Ok(())
    }
    pub async fn begin(
        &self,
        ctx: &Ctx,
        kind: &'static str,
        uri: &Uri,
        body: BodyIdentity<'_>,
    ) -> ApiResult<Option<Claim>> {
        self.begin_continuing(ctx, kind, uri, body, None).await
    }

    pub async fn begin_continuing(
        &self,
        ctx: &Ctx,
        kind: &'static str,
        uri: &Uri,
        body: BodyIdentity<'_>,
        resume: Option<Resume>,
    ) -> ApiResult<Option<Claim>> {
        let Some(key) = request_key(ctx)? else {
            return Ok(None);
        };
        let account = ctx.user()?.id();
        let hash = match body {
            BodyIdentity::Json(bytes) => body_hash(account, &key, bytes)?,
            BodyIdentity::Raw(input) => input.fingerprint(account, &key)?,
        };
        self.begin_identified(
            ctx,
            kind,
            uri,
            Identified {
                key,
                account,
                hash,
                resume,
            },
        )
        .await
    }
    pub async fn begin_proven(
        &self,
        ctx: &Ctx,
        kind: &'static str,
        uri: &Uri,
        bytes: &[u8],
        account: Option<Id>,
    ) -> ApiResult<(Option<Claim>, Option<Unassigned>)> {
        let Some(key) = request_key(ctx)? else {
            return Ok((None, None));
        };
        let hash = proof_body_hash(&key, bytes)?;
        if let Some(account) = account {
            return Ok((
                self.begin_identified(
                    ctx,
                    kind,
                    uri,
                    Identified {
                        key,
                        account,
                        hash,
                        resume: None,
                    },
                )
                .await?,
                None,
            ));
        }
        let now = ctx.app.clock.now();
        let mut conn = ctx.app.db.acquire().await?;
        if window_since(&mut conn, now).await? == now {
            return Ok((None, None));
        }
        Ok((
            None,
            Some(Unassigned {
                key,
                method: ctx.route.method.as_str().to_owned(),
                path: target(uri),
                record: Record::pending(hash, kind, new_id()),
                at: now,
            }),
        ))
    }
    /// A concurrent public first attempt may have lost the native uniqueness/setup race.
    /// Only an already committed matching record may replace that ordinary refusal.
    pub async fn find_proven(
        &self,
        ctx: &Ctx,
        kind: &'static str,
        uri: &Uri,
        bytes: &[u8],
        account: Id,
    ) -> ApiResult<Option<(Record, StatusCode)>> {
        let Some(key) = request_key(ctx)? else {
            return Ok(None);
        };
        let lease = self.lock(ctx.app.clone(), account, key.clone()).await?;
        let mut tx = ctx.app.db.begin().await?;
        let since = window_since(&mut tx, ctx.app.clock.now()).await?;
        let found = rows::creation::locked(&mut tx, account, &key, since).await?;
        let found = match found {
            Some(saved) => {
                let record = Record::read(saved.response).map_err(ApiError::internal)?;
                if saved.method != ctx.route.method.as_str()
                    || saved.path != target(uri)
                    || record.body_hash != proof_body_hash(&key, bytes)?
                    || record.native_kind != kind
                {
                    return Err(mismatch());
                }
                record
                    .outcome
                    .as_ref()
                    .map(|_| {
                        StatusCode::from_u16(
                            u16::try_from(saved.status).map_err(ApiError::internal)?,
                        )
                        .map(|status| (record.clone(), status))
                        .map_err(ApiError::internal)
                    })
                    .transpose()?
            }
            None => None,
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        lease.release().await;
        Ok(found)
    }
    async fn begin_identified(
        &self,
        ctx: &Ctx,
        kind: &'static str,
        uri: &Uri,
        identity: Identified,
    ) -> ApiResult<Option<Claim>> {
        let Identified {
            key,
            account,
            hash,
            resume,
        } = identity;
        let lease = self.lock(ctx.app.clone(), account, key.clone()).await?;
        let method = ctx.route.method.as_str().to_owned();
        let path = target(uri);
        let now = ctx.app.clock.now();
        let mut tx = ctx.app.db.begin().await?;
        let since = window_since(&mut tx, now).await?;
        if since == now {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            lease.release().await;
            return Ok(None);
        }
        // Keep this metadata transaction through the decision: a late native commit after
        // loss of a lock session must be observed rather than overwritten from an old read.
        if let Some(saved) = rows::creation::locked(&mut tx, account, &key, since).await? {
            if saved.method != method || saved.path != path {
                return Err(mismatch());
            }
            let mut record = Record::read(saved.response).map_err(ApiError::internal)?;
            if record.body_hash != hash || record.native_kind != kind {
                return Err(mismatch());
            }
            if let (Some(resume), Some(created)) = (resume, record.outcome.as_ref())
                && resume(ctx, created)?
            {
                // Only the recording owner changes. Accepted identities and the replay
                // window stay; the fresh fence rejects an old writer after a lost lease.
                record.owner = new_id();
                record.failure = None;
                sqlx::query("UPDATE idempotency_keys SET response=$3 WHERE user_id=$1 AND key=$2")
                    .bind(account)
                    .bind(&key)
                    .bind(serde_json::to_value(&record).map_err(ApiError::internal)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(orch_db::DbError::from)?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Ok(Some(Claim::New(Pending {
                    capture: Capture {
                        account,
                        key,
                        record,
                    },
                    method,
                    path,
                    lease,
                    resume: Some(resume),
                })));
            }
            if record.outcome.is_some() || record.failure.is_some() {
                tx.commit().await.map_err(orch_db::DbError::from)?;
                drop(lease);
                let status =
                    StatusCode::from_u16(u16::try_from(saved.status).map_err(ApiError::internal)?)
                        .map_err(ApiError::internal)?;
                return Ok(Some(Claim::Saved {
                    account,
                    record,
                    status,
                }));
            }
        }
        let record = Record::pending(hash, kind, new_id());
        rows::keep(
            &mut tx,
            account,
            &key,
            &Replay {
                method: method.clone(),
                path: path.clone(),
                status: i32::from(ctx.route.status),
                response: serde_json::to_value(&record).map_err(ApiError::internal)?,
            },
            now,
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Some(Claim::New(Pending {
            capture: Capture {
                account,
                key,
                record,
            },
            method,
            path,
            lease,
            resume,
        })))
    }
}

impl Pending {
    /// Read committed native progress while retaining this request's key lease and owner fence.
    pub async fn accepted(&self, ctx: &Ctx) -> ApiResult<Option<(Created, StatusCode)>> {
        let mut conn = ctx.app.db.acquire().await?;
        let (value, status): (Json, i32) = sqlx::query_as(
            "SELECT response,status FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(self.capture.account)
        .bind(&self.capture.key)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        let record = Record::read(value).map_err(ApiError::internal)?;
        if record.owner != self.capture.record.owner
            || record.body_hash != self.capture.record.body_hash
            || record.native_kind != self.capture.record.native_kind
        {
            return Err(ApiError::internal(
                "the creating request lost its recording owner; retry its key",
            ));
        }
        let status = StatusCode::from_u16(u16::try_from(status).map_err(ApiError::internal)?)
            .map_err(ApiError::internal)?;
        Ok(record.outcome.map(|created| (created, status)))
    }
    /// The native transaction saved its identity with the effect. An existing native
    /// client-id no-op can supply its same real identity from the typed answer too.
    pub async fn finish(
        self,
        ctx: &Ctx,
        status: StatusCode,
        identity: Option<Created>,
        failure: Option<ErrorCode>,
    ) -> ApiResult<()> {
        let mut tx = ctx.app.db.begin().await?;
        let saved: Option<(Json, i32)> = sqlx::query_as(
            "SELECT response,status FROM idempotency_keys WHERE user_id=$1 AND key=$2 FOR UPDATE",
        )
        .bind(self.capture.account)
        .bind(&self.capture.key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
        let (value, accepted_status) =
            saved.ok_or_else(|| ApiError::internal("the creation record disappeared"))?;
        let mut record = Record::read(value).map_err(ApiError::internal)?;
        if record.owner != self.capture.record.owner {
            return Err(ApiError::internal(
                "the creating request lost its recording owner; retry its key",
            ));
        }
        if let Some(identity) = identity {
            if identity.kind != record.native_kind
                || record
                    .outcome
                    .as_ref()
                    .is_some_and(|old| old.id != identity.id)
            {
                return Err(ApiError::internal(
                    "the native result differs from its committed identity",
                ));
            }
            if let Some(old) = &mut record.outcome {
                if let (Some(old), Some(extra)) =
                    (old.data.as_object_mut(), identity.data.as_object())
                {
                    for (key, value) in extra {
                        old.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                } else if old.data.is_null() {
                    old.data = identity.data;
                }
            } else {
                record.outcome = Some(identity);
            }
        }
        // Fail closed if an identity-free consumer omitted its native observer:
        // an accepted effect must never become a fresh attempt on the next retry.
        let unrecorded = failure.is_none() && record.outcome.is_none();
        let partial = match (self.resume, record.outcome.as_ref()) {
            (Some(resume), Some(created)) => resume(ctx, created)?,
            _ => false,
        };
        let status = if unrecorded {
            StatusCode::INTERNAL_SERVER_ERROR
        } else if partial && failure.is_some() {
            // New-work refusal does not replace the earlier native acceptance's status.
            StatusCode::from_u16(u16::try_from(accepted_status).map_err(ApiError::internal)?)
                .map_err(ApiError::internal)?
        } else {
            status
        };
        let failure = if unrecorded {
            Some(ErrorCode::Internal)
        } else {
            failure
        };
        record.failure = failure.map(|code| code.as_str().to_owned());
        sqlx::query("UPDATE idempotency_keys SET status=$3,response=$4 WHERE user_id=$1 AND key=$2 AND method=$5 AND path=$6")
            .bind(self.capture.account).bind(&self.capture.key).bind(i32::from(status.as_u16())).bind(serde_json::to_value(record).map_err(ApiError::internal)?)
            .bind(&self.method).bind(&self.path).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.lease.release().await;
        if unrecorded {
            Err(ApiError::internal(
                "the native creator did not retain its identity",
            ))
        } else {
            Ok(())
        }
    }
}

#[path = "idempotency/claims.rs"]
pub mod claims;

#[path = "idempotency/commands.rs"]
pub mod commands;

#[path = "idempotency/memory.rs"]
pub mod memory;

#[path = "idempotency/raw.rs"]
pub mod raw;

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use crate::middleware::authorization;

    use crate::middleware::transaction_guard;

    use crate::places;

    use crate::support::World;

    use crate::testing::TestApp;

    use orch_api_contract::personal::MeRecentCreate;

    use orch_api_contract::personal::MeRecentCreateBody;
    #[cfg(any(test, feature = "test-support"))]
    async fn partial_native_progress_resumes_under_the_same_key_and_a_completed_outcome_stays_final()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        fn producer<'a>(
            ctx: &'a Ctx,
            conn: &'a mut orch_db::Conn,
            caller: &'a orch_authz::Caller,
            (): (),
            _: MeRecentCreateBody,
        ) -> crate::registry::OperationFuture<'a> {
            Box::pin(async move {
                let account = crate::key_operations::targets::account(conn, caller.user()).await?;
                let mut plan = crate::registry::OperationPlan::new(ctx.route, ctx.route.name)?;
                plan.bind(0, account.object.kind, &[account])?;
                Ok(plan)
            })
        }
        let world = World::new().await;
        let maker = places::person(&world, "partial-progress-maker").await;
        let org = places::make_org(&world, &maker, "Two native visits", "two-native-visits")
            .await
            .base
            .id;
        let mut registry = crate::registry::Registry::new();
        registry.creation(MeRecentCreate, Registered::from(ObservedCreation::<(), MeRecentCreateBody, ()> {
            kind: "two_native_visits",
            replay: |ctx, (), _, created| Box::pin(async move {
                if ctx.operation_request().is_some() || created.data["complete"] != true {
                    return Err(ApiError::internal("a final native replay retained a writer or lost completion"));
                }
                Ok(())
            }),
        }).continuing(|_, created| Ok(created.data["complete"] == false)))
        .on_with_operation(MeRecentCreate, producer, |ctx: Ctx, (): (), body: MeRecentCreateBody| async move {
            let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
            authorization::require(&mut tx, guard.principal(), body.node_id, authorization::VIEW,
                orch_authz::Act::Use, orch_authz::Policy::default()).await?;
            let key = request_key(&ctx)?.ok_or_else(||ApiError::internal("the aggregate fixture needs its key"))?;
            let value: Json = sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2 FOR UPDATE")
                .bind(guard.user().id()).bind(&key).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
            let record = Record::read(value).map_err(ApiError::internal)?;
            let mut probe = ctx.app.db.acquire().await?;
            if rows::creation::try_lock(&mut probe, guard.user().id(), &key).await? {
                rows::creation::unlock(&mut probe, guard.user().id(), &key).await?;
                return Err(ApiError::internal("native continuation lost its still-owned key lease"));
            }
            drop(probe);
            let accepted = record.outcome.as_ref().and_then(|created|created.data["parts"].as_u64()).unwrap_or(0);
            if accepted >= 2 { return Err(ApiError::internal("completed native parts entered a writer again")); }
            orch_db::queries::hierarchy::touch_recent(&mut tx, guard.user().id(), body.node_id,
                guard.now().get(), i64::from(ctx.app.config.limits.recents_per_user)).await?;
            orch_db::idempotency::creation::observe_with(&mut tx, Some(guard.user().id()), "two_native_visits",
                body.node_id, serde_json::json!({"parts":accepted+1,"complete":accepted+1==2})).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok(())
        });
        let app = TestApp::over(world.app.app.clone(), world.app.clock.clone(), registry);
        let header = new_id().to_string();
        let send = || async {
            let call = app
                .call(MeRecentCreate, &[])
                .token(&maker.token)
                .header("Idempotency-Key", &header)
                .body(&MeRecentCreateBody { node_id: org });
            call.send_bytes().await
        };
        assert_eq!(send().await.status, StatusCode::NO_CONTENT);
        let before: (Json, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
            "SELECT response,created_at FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(maker.id())
        .bind(&header)
        .fetch_one(&world.db.pool)
        .await?;
        let first = Record::read(before.0.clone())?;
        assert_eq!(
            first.outcome.as_ref().map(|created| &created.data),
            Some(&serde_json::json!({"parts":1,"complete":false}))
        );
        world.advance(chrono::Duration::minutes(1));
        let (left, right) = tokio::join!(send(), send());
        assert_eq!(
            left.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&left.raw)
        );
        assert_eq!(
            right.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&right.raw)
        );
        let after: (Json, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
            "SELECT response,created_at FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(maker.id())
        .bind(&header)
        .fetch_one(&world.db.pool)
        .await?;
        let completed = Record::read(after.0.clone())?;
        assert_ne!(completed.owner, first.owner);
        assert_eq!(completed.body_hash, first.body_hash);
        assert_eq!(
            after.1, before.1,
            "continuation does not renew the replay window"
        );
        assert_eq!(
            completed.outcome.as_ref().map(|created| &created.data),
            Some(&serde_json::json!({"parts":2,"complete":true}))
        );
        assert_eq!(send().await.status, StatusCode::NO_CONTENT);
        let final_row: Json =
            sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
                .bind(maker.id())
                .bind(&header)
                .fetch_one(&world.db.pool)
                .await?;
        assert_eq!(final_row, after.0);
        let visits: f64 = sqlx::query_scalar(
            "SELECT visit_count FROM recents WHERE user_id=$1 AND kind='org' AND id=$2",
        )
        .bind(maker.id())
        .bind(org)
        .fetch_one(&world.db.pool)
        .await?;
        // The first visit's positive score decays below one across the minute; one
        // new visit is in (1,2], while repeating another part would exceed two.
        assert!(
            visits > 1.0 && visits <= 2.0,
            "the accepted first part and remaining second part each execute once: {visits}"
        );
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_partial_native_progress_resumes_under_the_same_key_and_a_completed_outcome_stays_final()
    -> ::futures::future::LocalBoxFuture<
        'static,
        Result<(), Box<dyn ::std::error::Error + Send + Sync>>,
    > {
        ::std::boxed::Box::pin(self::partial_native_progress_resumes_under_the_same_key_and_a_completed_outcome_stays_final())
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn a_completed_creation_never_installs_the_original_writer_request()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        fn producer<'a>(
            ctx: &'a Ctx,
            conn: &'a mut orch_db::Conn,
            caller: &'a orch_authz::Caller,
            (): (),
            _body: MeRecentCreateBody,
        ) -> crate::registry::OperationFuture<'a> {
            Box::pin(async move {
                let account = crate::key_operations::targets::account(conn, caller.user()).await?;
                let mut plan = crate::registry::OperationPlan::new(ctx.route, ctx.route.name)?;
                plan.bind(0, account.object.kind, &[account])?;
                Ok(plan)
            })
        }
        let world = World::new().await;
        let maker = places::person(&world, "writer-state-creator").await;
        let org = places::make_org(&world, &maker, "One recorded visit", "recorded-visit")
            .await
            .base
            .id;
        let mut registry = crate::registry::Registry::new();
        registry
            .creation(
                MeRecentCreate,
                ObservedCreation::<(), MeRecentCreateBody, ()> {
                    kind: "counter_recent",
                    replay: |ctx, (), _body, _created| {
                        Box::pin(async move {
                            if ctx.operation_request().is_some() {
                                return Err(ApiError::internal(
                                    "a completed replay retained its original writer",
                                ));
                            }
                            Ok(())
                        })
                    },
                },
            )
            .on_with_operation(
                MeRecentCreate,
                producer,
                |ctx: Ctx, (): (), body: MeRecentCreateBody| async move {
                    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
                    authorization::require(
                        &mut tx,
                        guard.principal(),
                        body.node_id,
                        authorization::VIEW,
                        orch_authz::Act::Use,
                        orch_authz::Policy::default(),
                    )
                    .await?;
                    orch_db::queries::hierarchy::touch_recent(
                        &mut tx,
                        guard.principal().user(),
                        body.node_id,
                        guard.now().get(),
                        i64::from(ctx.app.config.limits.recents_per_user),
                    )
                    .await?;
                    orch_db::idempotency::creation::observe(
                        &mut tx,
                        Some(guard.principal().user()),
                        "counter_recent",
                        body.node_id,
                    )
                    .await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    Ok(())
                },
            );
        let app = TestApp::over(world.app.app.clone(), world.app.clock.clone(), registry);
        let header = new_id().to_string();
        for _ in 0..2 {
            let reply = app
                .call(MeRecentCreate, &[])
                .token(&maker.token)
                .header("Idempotency-Key", &header)
                .body(&MeRecentCreateBody { node_id: org })
                .send_bytes()
                .await;
            assert_eq!(
                reply.status,
                StatusCode::NO_CONTENT,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
        }
        let count: f64 = sqlx::query_scalar(
            "SELECT visit_count FROM recents WHERE user_id=$1 AND kind='org' AND id=$2",
        )
        .bind(maker.id())
        .bind(org)
        .fetch_one(&world.db.pool)
        .await?;
        assert_eq!(count, 1.0);
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_completed_creation_never_installs_the_original_writer_request()
    -> ::futures::future::LocalBoxFuture<
        'static,
        Result<(), Box<dyn ::std::error::Error + Send + Sync>>,
    > {
        ::std::boxed::Box::pin(
            self::a_completed_creation_never_installs_the_original_writer_request(),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn an_identity_free_consumer_without_its_observer_fails_closed_instead_of_running_twice()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let world = World::new().await;
        let maker = places::person(&world, "unrecorded-creator").await;
        let org = places::make_org(&world, &maker, "One accepted counter", "one-counter")
            .await
            .base
            .id;
        let mut registry = crate::registry::Registry::new();
        registry
            .creation(
                MeRecentCreate,
                ObservedCreation::<(), MeRecentCreateBody, ()> {
                    kind: "deliberately_unobserved_recent",
                    replay: |_ctx, (), _body: MeRecentCreateBody, _created| {
                        Box::pin(async { Err(ApiError::internal("no identity was retained")) })
                    },
                },
            )
            .on(
                MeRecentCreate,
                |ctx: Ctx, (): (), body: MeRecentCreateBody| async move {
                    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
                    authorization::require(
                        &mut tx,
                        guard.principal(),
                        body.node_id,
                        authorization::VIEW,
                        orch_authz::Act::Use,
                        orch_authz::Policy::default(),
                    )
                    .await?;
                    // Deliberately bind a different native kind to simulate a missing observer.
                    orch_db::queries::hierarchy::touch_recent(
                        &mut tx,
                        guard.principal().user(),
                        body.node_id,
                        guard.now().get(),
                        i64::from(ctx.app.config.limits.recents_per_user),
                    )
                    .await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    Ok(())
                },
            );
        let app = TestApp::over(world.app.app.clone(), world.app.clock.clone(), registry);
        let key = new_id().to_string();
        for _ in 0..2 {
            let reply = app
                .call(MeRecentCreate, &[])
                .token(&maker.token)
                .header("Idempotency-Key", &key)
                .body(&MeRecentCreateBody { node_id: org })
                .send_bytes()
                .await;
            assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
        }
        let count: f64 = sqlx::query_scalar(
            "SELECT visit_count FROM recents WHERE user_id=$1 AND kind='org' AND id=$2",
        )
        .bind(maker.id())
        .bind(org)
        .fetch_one(&world.db.pool)
        .await?;
        assert_eq!(count, 1.0);
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_an_identity_free_consumer_without_its_observer_fails_closed_instead_of_running_twice()
    -> ::futures::future::LocalBoxFuture<
        'static,
        Result<(), Box<dyn ::std::error::Error + Send + Sync>>,
    > {
        ::std::boxed::Box::pin(self::an_identity_free_consumer_without_its_observer_fails_closed_instead_of_running_twice())
    }
}
