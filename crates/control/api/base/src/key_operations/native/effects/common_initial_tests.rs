//! Shared phase controls with real retained uploads, native source opening and HTTP identities.
//! The importing family owns its complete queue/writer and package-default selection tests.

use super::*;

use crate::middleware::authentication;

use crate::places::make_org;

use crate::places::person;

use crate::support::World;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyScope;

use orch_api_contract::imports::ImportAuthors;

use orch_api_contract::imports::ImportRequest;

use orch_api_contract::imports::ImportSource;

use orch_api_contract::imports::ImportSourceFile;

use orch_api_contract::imports::NodeImportCreate;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::surface::HttpSurface;

use orch_db::queries::identity;

use orch_db::queries::uploads;

use orch_engine::imports::Opened;

use orch_engine::imports::Plan;

use orch_types::new_id;

use std::sync::atomic::AtomicUsize;

use tokio::sync::Notify;

use orch_api_contract::import_creation_sources::CANDIDATES;

use orch_api_contract::import_creation_sources::Source;

use orch_api_contract::upload_sources;
pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const UPLOAD: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Upload);

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

const ROWS: &[Declaration<Source>] = &CANDIDATES;

struct Input {
    request: ImportRequest,
    metadata: AtomicBool,
    authorizations: AtomicUsize,
    metadata_opened: Notify,
    metadata_resume: Notify,
    opened: Notify,
    resume: Notify,
    calls: AtomicUsize,
}

struct Fetched {
    input: Arc<Input>,
    opened: Arc<Opened>,
}

struct Ready {
    plan: Plan,
}

struct Inspect;

fn upload_facts(user: Id) -> Vec<Facts<Source>> {
    ROWS[0]
        .bindings
        .iter()
        .take(upload_sources::BLOB_GET.bindings.len())
        .map(|binding| {
            Facts::collection(
                binding.source,
                if binding.source == Source::Bytes(upload_sources::Source::ReadUploads) {
                    vec![(
                        UPLOAD,
                        vec![KeyResolvedTarget {
                            object: KeyObjectRef {
                                kind: ACCOUNT,
                                id: Some(user),
                            },
                            node_ancestors: vec![],
                            document_ancestors: vec![],
                            credential_account: None,
                        }],
                    )]
                } else {
                    Vec::new()
                },
            )
        })
        .collect()
}

impl Effect for Inspect {
    type Source = Source;
    type Query = ();
    type Body = Arc<Input>;
    type Value = Ready;
    type Output = ();
    fn declaration(ready: &Ready) -> &'static Declaration<Source> {
        &ROWS[usize::from(
            ready.plan.access.restored || ready.plan.authors == ImportAuthors::Accounts,
        )]
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = common_initial_value::<Self>(ctx)?;
            if !Arc::ptr_eq(&body, &fetched.input) {
                return Err(declaration_error());
            }
            let facts = upload_facts(caller.user());
            let common = orch_engine::operation_scope::sources::common::Common::bind(
                ctx.route, ROWS, &facts,
            )
            .map_err(admission::binding_error)?;
            let read = common
                .preparation_caller(caller)
                .map_err(admission::binding_error)?;
            let principal = authorization::load_principal(conn, read, ctx.app.clock.time()).await?;
            crate::attachments::require(conn, &principal, &body.request.source.files[0].blob)
                .await?;
            let node = ctx.id("id")?;
            let plan = orch_engine::imports::build(
                &ctx.app.engine,
                conn,
                &principal,
                node,
                &body.request,
                Arc::clone(&fetched.opened),
            )
            .await
            .map_err(ApiError::internal)?;
            let target = crate::key_operations::targets::node_identity(conn, node).await?;
            let mut facts = facts;
            facts.push(Facts::collection(
                Source::Destinations,
                vec![(target.object.kind, vec![target.clone()])],
            ));
            if plan.access.restored || plan.authors == ImportAuthors::Accounts {
                facts.push(Facts::collection(
                    Source::Delegation,
                    vec![(target.object.kind, vec![target])],
                ));
            }
            Ok((Ready { plan }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        ready: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.app
                .engine
                .import_place_seen(conn, principal, ready.plan.destination.id)
                .await
                .map_err(ApiError::internal)
        })
    }
}

impl CommonInitial for Inspect {
    type Input = Arc<Input>;
    type Fetched = Fetched;
    const CANDIDATES: &'static [Declaration<Source>] = ROWS;
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<(Arc<Input>, Vec<Facts<Source>>)>> {
        Box::pin(async move {
            let attempt = body.authorizations.fetch_add(1, Ordering::SeqCst);
            if body.metadata.load(Ordering::SeqCst) {
                if attempt == 0 {
                    body.metadata_opened.notify_one();
                    body.metadata_resume.notified().await;
                }
                let name: String =
                    sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1 FOR SHARE")
                        .bind(ctx.id("id")?)
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                assert_eq!(name, "Changed native metadata");
            }
            Ok((body, upload_facts(caller.user())))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            crate::attachments::require(conn, principal, &input.request.source.files[0].blob).await
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            assert!(caller.admitted_required_roles().is_none());
            input.calls.fetch_add(1, Ordering::SeqCst);
            let mut conn = ctx.app.db.acquire().await?;
            let limits = ctx
                .app
                .engine
                .import_limits(&mut conn, ctx.id("id")?)
                .await
                .map_err(ApiError::internal)?;
            drop(conn);
            let opened = Arc::new(
                orch_engine::imports::open(&ctx.app.engine, &input.request.source, &limits)
                    .await
                    .map_err(ApiError::internal)?,
            );
            input.opened.notify_one();
            input.resume.notified().await;
            Ok(Fetched { input, opened })
        })
    }
}

struct Fixture {
    world: World,
    ctx: Ctx,
    bearer: String,
    token: Option<Id>,
    input: Arc<Input>,
    owner: Id,
    org: Id,
}

fn scope(owner: Id, org: Id, write: bool) -> TestResult<KeyScope> {
    Ok(serde_json::from_value(serde_json::json!({ "rules": [
        { "target": {"kind":"account", "id":owner, "reach":"self"}, "resources":["upload"], "allow":[
            {"operation": ROWS[0].operation, "actions":["read"]}, {"operation": ROWS[1].operation, "actions":["read"]}
        ]},
        { "target": {"kind":"org", "id":org, "reach":"self"}, "resources":["org"], "allow":[
            {"operation": ROWS[0].operation, "actions": if write { vec!["write"] } else { vec!["read"] }},
            {"operation": ROWS[1].operation, "actions": if write { vec!["write","manage"] } else { vec!["read"] }}
        ]}
    ] }))?)
}

impl Fixture {
    async fn new(session: bool) -> TestResult<Self> {
        let world = World::new().await;
        let who = person(&world, "common-reader").await;
        let org = make_org(&world, &who, "Common initial", "common-initial")
            .await
            .base
            .id;
        let bytes = bytes::Bytes::from_static(b"# Native import source\n");
        let meta = world.app.app.blobs.put_bytes(bytes).await?;
        let mut conn = world.conn().await;
        world
            .app
            .app
            .blobs
            .register(&mut conn, meta, world.now())
            .await?;
        uploads::remember(&mut conn, who.id(), &meta.hash, world.now()).await?;
        let (bearer, token, surface) = if session {
            sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                .bind(authentication::token_hash(&who.token))
                .bind(world.now() + chrono::Duration::seconds(5))
                .execute(&mut *conn)
                .await?;
            (who.token.clone(), None, HttpSurface::Browser)
        } else {
            let bearer = authentication::new_token(authentication::API_TOKEN_PREFIX);
            let token = new_id();
            identity::create_api_token_scoped(
                &mut conn,
                &identity::NewApiToken {
                    id: token,
                    user_id: who.id(),
                    name: "Common native source",
                    token_hash: &authentication::token_hash(&bearer),
                    scopes: &[],
                    node_id: None,
                    at: world.now(),
                    expires_at: Some(world.now() + chrono::Duration::hours(1)),
                },
                Some(&serde_json::json!(scope(who.id(), org, true)?)),
            )
            .await?;
            (bearer, Some(token), HttpSurface::Key)
        };
        drop(conn);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {bearer}"))?,
        );
        let user =
            authentication::authenticate_on(surface, &world.app.app, &headers, world.now()).await?;
        let mut ctx = Ctx::for_tool(world.app.app.clone(), user, NodeImportCreate::ROUTE, org);
        ctx.surface = Some(surface);
        ctx.headers = headers;
        let input = Arc::new(Input {
            request: ImportRequest {
                client_id: "common-initial-control".into(),
                source: ImportSource {
                    files: vec![ImportSourceFile {
                        name: "source.md".into(),
                        blob: meta.hash,
                    }],
                    mode: None,
                },
                destination: Default::default(),
                options: orch_api_contract::imports::ImportOptions {
                    access: Some(false),
                    authors: Some(ImportAuthors::Labels),
                    ..Default::default()
                },
                choices: Default::default(),
            },
            metadata: AtomicBool::new(false),
            authorizations: AtomicUsize::new(0),
            metadata_opened: Notify::new(),
            metadata_resume: Notify::new(),
            opened: Notify::new(),
            resume: Notify::new(),
            calls: AtomicUsize::new(0),
        });
        Ok(Self {
            owner: who.id(),
            org,
            world,
            ctx,
            bearer,
            token,
            input,
        })
    }
    fn start(&self) -> tokio::task::JoinHandle<ApiResult<()>> {
        let ctx = self.ctx.clone();
        let input = Arc::clone(&self.input);
        tokio::spawn(async move {
            let caller = authorization::caller_of(ctx.user()?)?;
            initial_common::<Inspect>(&ctx, &caller, (), input).await
        })
    }
    async fn opened(&self) -> TestResult {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.input.opened.notified(),
        )
        .await?;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn common_initial_retries_a_real_metadata_snapshot_conflict_without_reopening_native_io()
-> TestResult {
    let s = Fixture::new(false).await?;
    s.input.metadata.store(true, Ordering::SeqCst);
    let started = s.start();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        s.input.metadata_opened.notified(),
    )
    .await?;
    sqlx::query("UPDATE nodes SET name='Changed native metadata' WHERE id=$1")
        .bind(s.org)
        .execute(&s.world.db.pool)
        .await?;
    s.input.metadata_resume.notify_one();
    s.opened().await?;
    s.input.resume.notify_one();
    started.await??;
    s.start().await??;
    assert_eq!(s.input.authorizations.load(Ordering::SeqCst), 2);
    assert_eq!(s.input.calls.load(Ordering::SeqCst), 1);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_initial_retries_a_real_metadata_snapshot_conflict_without_reopening_native_io()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::common_initial_retries_a_real_metadata_snapshot_conflict_without_reopening_native_io(
        ),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn common_initial_opens_real_upload_once_for_concurrent_callers_and_pure_repreparation()
-> TestResult {
    let s = Fixture::new(false).await?;
    let first = s.start();
    s.opened().await?;
    let second = s.start();
    tokio::task::yield_now().await;
    s.input.resume.notify_one();
    first.await??;
    second.await??;
    let one = common_initial_value::<Inspect>(&s.ctx)?;
    let two = common_initial_value::<Inspect>(&s.ctx)?;
    assert!(Arc::ptr_eq(&one, &two));
    let caller = authorization::caller_of(s.ctx.user()?)?;
    for _ in 0..2 {
        let mut tx = s.ctx.app.db.begin_snapshot().await?;
        let prepared =
            prepare::<Inspect>(&s.ctx, &mut tx, &caller, (), Arc::clone(&s.input)).await?;
        assert_eq!(prepared.variant().operation, ROWS[0].operation);
        let accepted = prepared.admit(&caller)?.caller;
        assert_eq!(
            orch_authz::CapturedOperation::capture(&accepted)?.operation(),
            ROWS[0].operation
        );
        tx.commit().await?;
    }
    assert_eq!(s.input.calls.load(Ordering::SeqCst), 1);
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM imports")
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(jobs, 0);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_initial_opens_real_upload_once_for_concurrent_callers_and_pure_repreparation()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::common_initial_opens_real_upload_once_for_concurrent_callers_and_pure_repreparation(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn common_initial_rechecks_raw_identity_and_keeps_original_access_deadline_after_native_wait()
-> TestResult {
    for mode in ["unchanged", "digest", "raw_expiry", "raw_extension"] {
        let s = Fixture::new(mode.starts_with("raw_")).await?;
        let pending = s.start();
        s.opened().await?;
        if mode == "digest" {
            sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                .bind(s.token.ok_or("missing key")?)
                .bind(authentication::token_hash("replacement bearer"))
                .execute(&s.world.db.pool)
                .await?;
        } else if mode.starts_with("raw_") {
            sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                .bind(authentication::token_hash(&s.bearer))
                .bind(s.world.now() + chrono::Duration::hours(1))
                .execute(&s.world.db.pool)
                .await?;
            if mode == "raw_expiry" {
                s.world.advance(chrono::Duration::seconds(6));
            }
        }
        s.input.resume.notify_one();
        let result = pending.await?;
        assert_eq!(
            result.is_ok(),
            matches!(mode, "unchanged" | "raw_extension"),
            "{mode}"
        );
        if matches!(mode, "digest" | "raw_expiry") {
            assert!(s.start().await?.is_err());
        }
        if mode == "raw_extension" {
            let caller = authorization::caller_of(s.ctx.user()?)?;
            let mut tx = s.ctx.app.db.begin_snapshot().await?;
            let prepared =
                prepare::<Inspect>(&s.ctx, &mut tx, &caller, (), Arc::clone(&s.input)).await?;
            let accepted = prepared.admit(&caller)?.caller;
            let selected = orch_authz::CapturedOperation::capture(&accepted)?;
            let captured =
                super::super::http_origin::capture_operation(&s.ctx, &mut tx, &selected).await?;
            assert_eq!(captured.origin().operation(), ROWS[0].operation);
            assert_eq!(
                captured.until().map(|end| end.get()),
                Some(s.world.now() + chrono::Duration::seconds(5))
            );
            tx.commit().await?;
            s.world.advance(chrono::Duration::seconds(6));
            let mut tx = s.ctx.app.db.begin_snapshot().await?;
            assert!(
                captured
                    .current(&mut tx, s.ctx.app.clock.as_ref())
                    .await
                    .is_err()
            );
        }
        assert_eq!(s.input.calls.load(Ordering::SeqCst), 1);
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_initial_rechecks_raw_identity_and_keeps_original_access_deadline_after_native_wait()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(self::common_initial_rechecks_raw_identity_and_keeps_original_access_deadline_after_native_wait())
}

#[cfg(any(test, feature = "test-support"))]
async fn common_initial_keeps_observed_key_narrowing_even_when_policy_broadens_before_resolution()
-> TestResult {
    let s = Fixture::new(false).await?;
    let pending = s.start();
    s.opened().await?;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(serde_json::json!(scope(s.owner, s.org, false)?))
        .execute(&s.world.db.pool)
        .await?;
    s.input.resume.notify_one();
    pending.await??;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(serde_json::json!(scope(s.owner, s.org, true)?))
        .execute(&s.world.db.pool)
        .await?;
    let caller = authorization::caller_of(s.ctx.user()?)?;
    let mut tx = s.ctx.app.db.begin_snapshot().await?;
    // The native archive stays open, but its now-resolved effect has no accepted Write.
    let prepared = prepare::<Inspect>(&s.ctx, &mut tx, &caller, (), Arc::clone(&s.input)).await;
    assert!(prepared.is_err());
    assert_eq!(s.input.calls.load(Ordering::SeqCst), 1);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_initial_keeps_observed_key_narrowing_even_when_policy_broadens_before_resolution()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(self::common_initial_keeps_observed_key_narrowing_even_when_policy_broadens_before_resolution())
}

#[cfg(any(test, feature = "test-support"))]
async fn common_initial_does_not_retry_a_failed_native_source_open() -> TestResult {
    let s = Fixture::new(false).await?;
    let hash = s.input.request.source.files[0].blob;
    assert!(s.world.app.app.blobs.store().delete(&hash).await?);
    assert!(s.start().await?.is_err());
    assert!(s.start().await?.is_err());
    assert_eq!(s.input.calls.load(Ordering::SeqCst), 1);
    assert!(common_initial_value::<Inspect>(&s.ctx).is_err());
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_initial_does_not_retry_a_failed_native_source_open()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(self::common_initial_does_not_retry_a_failed_native_source_open())
}

#[cfg(any(test, feature = "test-support"))]
async fn common_initial_keeps_the_ordinary_upload_custody_gate_before_native_io() -> TestResult {
    let s = Fixture::new(false).await?;
    let other = person(&s.world, "other-uploader").await;
    sqlx::query("UPDATE completed_uploads SET user_id=$3 WHERE user_id=$1 AND hash=$2")
        .bind(s.owner)
        .bind(s.input.request.source.files[0].blob.as_bytes().as_slice())
        .bind(other.id())
        .execute(&s.world.db.pool)
        .await?;
    assert!(s.start().await?.is_err());
    assert_eq!(s.input.calls.load(Ordering::SeqCst), 0);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_initial_keeps_the_ordinary_upload_custody_gate_before_native_io()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::common_initial_keeps_the_ordinary_upload_custody_gate_before_native_io(),
    )
}

struct CurrentUpload;

impl super::super::continuing::Source for CurrentUpload {
    type Role = Source;
    type Input = Arc<Input>;
    type Value = orch_types::Hash;
    const ROUTE: orch_api_contract::routes::Route = NodeImportCreate::ROUTE;
    const DECLARATION: &'static Declaration<Source> = &ROWS[0];
    fn prepare<'a>(
        _: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Arc<Input>,
        _: orch_types::Time,
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Source>>), super::super::continuing::Error>>
    {
        Box::pin(async move {
            Ok((
                input.request.source.files[0].blob,
                upload_facts(caller.user()),
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Arc<Input>,
        hash: &'a orch_types::Hash,
    ) -> BoxFuture<'a, Result<(), super::super::continuing::Error>> {
        Box::pin(async move {
            crate::attachments::require(conn, principal, hash)
                .await
                .map_err(|error| {
                    super::super::continuing::Error::Native(orch_engine::EngineError::BeforeEffect(
                        Box::new(error),
                    ))
                })
        })
    }
    fn locks<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Arc<Input>,
        _: &'a orch_types::Hash,
    ) -> BoxFuture<'a, Result<(), super::super::continuing::Error>> {
        Box::pin(async { Ok(()) })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Arc<Input>,
        old: &'a orch_types::Hash,
        fresh: &'a orch_types::Hash,
    ) -> BoxFuture<'a, Result<bool, super::super::continuing::Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn common_preparation_rechecks_native_custody_without_a_captured_operation() -> TestResult {
    use super::super::continuing::common;
    let s = Fixture::new(false).await?;
    let caller = authorization::caller_of(s.ctx.user()?)?;
    let mut tx = s.ctx.app.db.begin().await?;
    let (frame, deadline) = common::prepare_initial_on::<Inspect, CurrentUpload>(
        &s.ctx,
        &mut tx,
        &caller,
        Arc::clone(&s.input),
    )
    .await?;
    assert!(frame.principal().caller().admitted_operation().is_none());
    assert!(orch_authz::CapturedOperation::capture(frame.principal().caller()).is_err());
    assert_eq!(*frame.value(), s.input.request.source.files[0].blob);
    frame.check_deadline(s.ctx.app.clock.as_ref())?;
    let parts = frame.into_parts();
    assert_eq!(parts.principal.user(), s.owner);
    assert!(Arc::ptr_eq(parts.input.as_ref(), &s.input));
    assert_eq!(*parts.value, s.input.request.source.files[0].blob);
    deadline.check()?;
    tx.commit().await?;
    deadline.check()?;
    let other = person(&s.world, "moved-preparation-source").await;
    sqlx::query("UPDATE completed_uploads SET user_id=$3 WHERE user_id=$1 AND hash=$2")
        .bind(s.owner)
        .bind(parts.value.as_bytes().as_slice())
        .bind(other.id())
        .execute(&s.world.db.pool)
        .await?;
    let mut tx = s.ctx.app.db.begin().await?;
    assert!(
        common::refresh_initial_on::<Inspect, CurrentUpload>(&s.ctx, &mut tx, &parts.opened)
            .await
            .is_err()
    );
    assert_eq!(s.input.calls.load(Ordering::SeqCst), 0);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_preparation_rechecks_native_custody_without_a_captured_operation()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::common_preparation_rechecks_native_custody_without_a_captured_operation(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn common_preparation_keeps_observed_policy_and_raw_lifetime_minima() -> TestResult {
    use super::super::continuing::common;
    let s = Fixture::new(false).await?;
    let caller = authorization::caller_of(s.ctx.user()?)?;
    let mut tx = s.ctx.app.db.begin().await?;
    let (frame, deadline) = common::prepare_initial_on::<Inspect, CurrentUpload>(
        &s.ctx,
        &mut tx,
        &caller,
        Arc::clone(&s.input),
    )
    .await?;
    frame.check_deadline(s.ctx.app.clock.as_ref())?;
    let parts = frame.into_parts();
    tx.commit().await?;
    deadline.check()?;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(serde_json::to_value(scope(s.owner, s.org, false)?)?)
        .execute(&s.world.db.pool)
        .await?;
    let mut tx = s.ctx.app.db.begin().await?;
    let (narrowed, cut) =
        common::refresh_initial_on::<Inspect, CurrentUpload>(&s.ctx, &mut tx, &parts.opened)
            .await?;
    let opened = narrowed.into_opened();
    tx.commit().await?;
    cut.check()?;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(serde_json::to_value(scope(s.owner, s.org, true)?)?)
        .execute(&s.world.db.pool)
        .await?;
    let mut tx = s.ctx.app.db.begin().await?;
    let retained = s
        .ctx
        .publication
        .retained_ceiling()?
        .ok_or("missing accepted ceiling")?
        .restore_current(&mut tx, s.ctx.app.clock.as_ref())
        .await?
        .ok_or("ended ceiling")?;
    let destination = crate::key_operations::targets::node_identity(&mut tx, s.org).await?;
    let mut facts = upload_facts(s.owner);
    facts.push(Facts::collection(
        Source::Destinations,
        vec![(destination.object.kind, vec![destination])],
    ));
    let complete = bind_required(&s.ctx, &ROWS[0], &facts)?;
    assert!(
        complete.admit(&retained).is_err(),
        "observed Write loss survives broadening"
    );
    drop(tx);
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await?;
    let mut tx = s.ctx.app.db.begin().await?;
    assert!(
        common::refresh_initial_on::<Inspect, CurrentUpload>(&s.ctx, &mut tx, &opened)
            .await
            .is_err()
    );
    drop(tx);
    let s = Fixture::new(true).await?;
    let caller = authorization::caller_of(s.ctx.user()?)?;
    let mut tx = s.ctx.app.db.begin().await?;
    let (frame, deadline) = common::prepare_initial_on::<Inspect, CurrentUpload>(
        &s.ctx,
        &mut tx,
        &caller,
        Arc::clone(&s.input),
    )
    .await?;
    let opened = frame.into_opened();
    tx.commit().await?;
    s.world.advance(chrono::Duration::seconds(6));
    assert!(deadline.check().is_err());
    let mut tx = s.ctx.app.db.begin().await?;
    assert!(
        common::refresh_initial_on::<Inspect, CurrentUpload>(&s.ctx, &mut tx, &opened)
            .await
            .is_err()
    );
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_common_preparation_keeps_observed_policy_and_raw_lifetime_minima()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::key_operations::native::effects::common_initial_tests::TestResult,
> {
    ::std::boxed::Box::pin(self::common_preparation_keeps_observed_policy_and_raw_lifetime_minima())
}

#[path = "common_initial_tests/transactions.rs"]
pub mod transactions;
