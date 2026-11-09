//! Actual Mount creation, new credential, same singleton factory/styles and metadata validation.

use super::*;

use crate::middleware::authentication;

use crate::places::make_org;

use crate::places::person;

use crate::support::World;

use native::resource_reply;

use native::resource_reply::ResourceProjection;

use native::resource_reply::StoredResource;

use orch_access_contract::key_scope::KeyScope;

use orch_api_contract::mounts::MountCreate;

use orch_api_contract::mounts::MountCreateBody;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::surface::HttpSurface;

use orch_db::queries::identity;

use orch_types::new_id;

use std::sync::Arc;
pub type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: World,
    owner: Id,
    org: Id,
    mount: Id,
}

impl Fixture {
    async fn new() -> Proof<Self> {
        let world = World::new().await;
        let maker = person(&world, "singleton-maker").await;
        let org = make_org(&world, &maker, "Singleton root", "singleton-root")
            .await
            .base
            .id;
        let made = world
            .app
            .call(MountCreate, &[])
            .token(&maker.token)
            .body(&MountCreateBody {
                name: "initial-singleton".into(),
                description: None,
                home_id: Some(org),
                target: "/data/replay".into(),
                read_only: None,
                default_fate: None,
                access: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "ordinary creation returned no mount")?;
        sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)").bind(authentication::token_hash(&maker.token)).bind(world.now()).execute(&world.db.pool).await?;
        Ok(Self {
            owner: maker.id(),
            org,
            mount: made.base.id,
            world,
        })
    }
    async fn current(&self, operation: &str, read: bool, retained: bool) -> Proof<(Ctx, Id)> {
        let secret = authentication::new_token(authentication::API_TOKEN_PREFIX);
        let id = new_id();
        let (kind, target, actions) = if read {
            ("mount", self.mount, vec!["read"])
        } else {
            ("org", self.org, vec!["write"])
        };
        let scope: KeyScope = serde_json::from_value(serde_json::json!({"rules":[
            {"target":{"kind":kind,"id":target,"reach":"self"},"resources":["mount"],"allow":[{"operation":operation,"actions":actions}]},
            {"target":{"kind":"org","id":self.org,"reach":"self"},"resources":["org"],"allow":[{"operation":operation,"actions":["read"]}]}
        ]}))?;
        identity::create_api_token_scoped(
            &mut *self.world.conn().await,
            &identity::NewApiToken {
                id,
                user_id: self.owner,
                name: "current-singleton reader",
                token_hash: &authentication::token_hash(&secret),
                scopes: &[],
                node_id: None,
                at: self.world.now(),
                expires_at: Some(self.world.now() + chrono::Duration::minutes(10)),
            },
            Some(&serde_json::json!(scope)),
        )
        .await?;
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {secret}"))?,
        );
        let user = authentication::authenticate_on(
            HttpSurface::Key,
            &self.world.app.app,
            &headers,
            self.world.now(),
        )
        .await?;
        let mut ctx = Ctx::for_tool(
            self.world.app.app.clone(),
            user,
            MountCreate::ROUTE,
            self.mount,
        );
        ctx.surface = Some(HttpSurface::Key);
        ctx.headers = headers;
        if retained {
            ctx.retain_source(self.created())?;
        }
        Ok((ctx, id))
    }
    fn created(&self) -> Created {
        Created {
            id: self.mount,
            home: Some(self.org),
        }
    }
    async fn count(&self) -> Proof<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM mounts WHERE owner_id=$1")
                .bind(self.owner)
                .fetch_one(&self.world.db.pool)
                .await?,
        )
    }
}

// Both entry forms delegate to the actual same domain loader and native ordinary Read gate.
struct ByInput;

impl ResourceProjection for ByInput {
    type Source = Source;
    type Value = Subject;
    type Input = Created;
    const READ_SOURCE: Source = Source::Created;
    const ROLES: &'static single::Roles = &ROLES;
    fn declaration(_: &Ctx, _: &Created) -> ApiResult<&'static Declaration<Source>> {
        Ok(&sources::CREATE)
    }
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Created,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            ctx.retain_source(*input)?;
            <Mounts as native::Stored>::load(ctx, conn, &sources::CREATE).await
        })
    }
    fn object(value: &Subject) -> ObjectRef {
        <Mounts as StoredResource>::object(value)
    }
    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        <Mounts as StoredResource>::require_read(ctx, conn, principal, value)
    }
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        opened: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        <Mounts as StoredResource>::check_opened(ctx, conn, principal, opened, fresh)
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn singleton_completed_replay_uses_new_credential_native_current_style_and_spent_effects_are_absent()
-> Proof {
    let s = Fixture::new().await?;
    sqlx::query("UPDATE mounts SET name='current-singleton',description='Current native metadata' WHERE id=$1").bind(s.mount).execute(&s.world.db.pool).await?;
    let before = s.count().await?;
    let (stored, _) = s.current("MountCreate", true, true).await?;
    let full = resource_reply::stored_completed_replay::<Mounts>(&stored, s.owner)
        .await?
        .ok_or("same-operation singleton Read closed")?;
    let Resource::Mount(mount) = full else {
        return Err("native singleton style changed".into());
    };
    assert_eq!(mount.base.id, s.mount);
    assert_eq!(mount.base.name, "current-singleton");
    assert_eq!(mount.description, "Current native metadata");
    let (input, _) = s.current("MountCreate", true, false).await?;
    let full = resource_reply::with_input_completed_replay::<ByInput>(&input, s.owner, s.created())
        .await?
        .ok_or("input singleton Read closed")?;
    assert!(
        matches!(full,Resource::Mount(value) if value.base.id==s.mount && value.base.name=="current-singleton")
    );
    crate::key_operations::publication::check(&stored).await?;
    crate::key_operations::publication::check(&input).await?;
    assert_eq!(s.count().await?, before);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_singleton_completed_replay_uses_new_credential_native_current_style_and_spent_effects_are_absent()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof,
> {
    ::std::boxed::Box::pin(self::singleton_completed_replay_uses_new_credential_native_current_style_and_spent_effects_are_absent())
}

#[cfg(any(test, feature = "test-support"))]
async fn singleton_completed_replay_keeps_receipts_account_boundary_and_current_publication_cut()
-> Proof {
    let s = Fixture::new().await?;
    let before = s.count().await?;
    for (operation, read) in [("MountCreate", false), ("MountGet", true)] {
        let (ctx, _) = s.current(operation, read, true).await?;
        assert!(
            resource_reply::stored_completed_replay::<Mounts>(&ctx, s.owner)
                .await?
                .is_none()
        );
    }
    let (ctx, key) = s.current("MountCreate", true, true).await?;
    assert_eq!(
        resource_reply::stored_completed_replay::<Mounts>(&ctx, new_id())
            .await
            .err()
            .ok_or("foreign account replay admitted")?
            .problem
            .status,
        404
    );
    assert!(
        resource_reply::stored_completed_replay::<Mounts>(&ctx, s.owner)
            .await?
            .is_some()
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key)
        .bind(serde_json::json!({"rules":[]}))
        .execute(&s.world.db.pool)
        .await?;
    assert!(
        crate::key_operations::publication::check(&ctx)
            .await
            .is_err()
    );
    assert_eq!(s.count().await?, before);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_singleton_completed_replay_keeps_receipts_account_boundary_and_current_publication_cut()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof,
> {
    ::std::boxed::Box::pin(self::singleton_completed_replay_keeps_receipts_account_boundary_and_current_publication_cut())
}

struct AuxiliaryInput {
    created: Created,
    state: Arc<AuxiliaryState>,
}

struct AuxiliaryState {
    calls: std::sync::atomic::AtomicUsize,
    clock: orch_types::ManualClock,
    expire: bool,
}

struct WithAuxiliary;

impl ResourceProjection for WithAuxiliary {
    type Source = Source;
    type Value = (Subject, Arc<AuxiliaryState>);
    type Input = AuxiliaryInput;
    const READ_SOURCE: Source = Source::Created;
    const ROLES: &'static single::Roles = &ROLES;
    fn declaration(ctx: &Ctx, input: &AuxiliaryInput) -> ApiResult<&'static Declaration<Source>> {
        ByInput::declaration(ctx, &input.created)
    }
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a AuxiliaryInput,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (value, facts) = ByInput::load(ctx, conn, &input.created).await?;
            Ok(((value, Arc::clone(&input.state)), facts))
        })
    }
    fn object(value: &Self::Value) -> ObjectRef {
        ByInput::object(&value.0)
    }
    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        ByInput::require_read(ctx, conn, principal, &value.0)
    }
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        ByInput::check_opened(ctx, conn, principal, &old.0, &fresh.0)
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        value: &'a Self::Value,
        read: &'a resource_reply::ResponseRead,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            value
                .1
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(read.object(), ByInput::object(&value.0));
            let mut tx = ctx.app.db.begin().await?;
            // This is the real native row footprint. The shared checkpoint runs AFTER it.
            sqlx::query("SELECT id FROM mounts WHERE id=$1 FOR UPDATE")
                .bind(read.object().id)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
            let current = read.prepare_on(&mut tx).await?;
            assert_eq!(
                current.principal().caller().admitted_operation(),
                Some("MountCreate")
            );
            assert!(
                current
                    .principal()
                    .caller()
                    .admitted_required_roles()
                    .is_none()
            );
            assert!(
                current
                    .principal()
                    .caller()
                    .admitted_facets()
                    .ok_or_else(|| ApiError::internal("missing output Read"))?
                    .iter()
                    .all(|facet| facet.action == orch_access_contract::key_scope::KeyAction::Read)
            );
            if value.1.expire {
                value.1.clock.advance(chrono::Duration::minutes(11));
            }
            current.check()?;
            // No synthetic maintenance or successful writer is manufactured here. The
            // domain's existing migration consumes this check at its own first SQL.
            tx.commit().await.map_err(DbError::from)?;
            current.check()
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn singleton_auxiliary_read_uses_current_output_scope_after_native_locks_once() -> Proof {
    let s = Fixture::new().await?;
    let before = s.count().await?;
    let (ctx, _) = s.current("MountCreate", true, false).await?;
    let state = Arc::new(AuxiliaryState {
        calls: std::sync::atomic::AtomicUsize::new(0),
        clock: s.world.app.clock.clone(),
        expire: false,
    });
    let full = resource_reply::with_input_completed_replay::<WithAuxiliary>(
        &ctx,
        s.owner,
        AuxiliaryInput {
            created: s.created(),
            state: Arc::clone(&state),
        },
    )
    .await?;
    assert!(matches!(full,Some(Resource::Mount(value)) if value.base.id==s.mount));
    crate::key_operations::publication::check(&ctx).await?;
    crate::key_operations::publication::check(&ctx).await?;
    assert_eq!(state.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(s.count().await?, before);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_singleton_auxiliary_read_uses_current_output_scope_after_native_locks_once()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof,
> {
    ::std::boxed::Box::pin(
        self::singleton_auxiliary_read_uses_current_output_scope_after_native_locks_once(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn singleton_auxiliary_read_keeps_first_sql_lifetime_and_does_not_run_without_response_read()
-> Proof {
    for expire in [false, true] {
        let s = Fixture::new().await?;
        let before = s.count().await?;
        let (ctx, _) = s
            .current(if expire { "MountCreate" } else { "MountGet" }, true, false)
            .await?;
        let state = Arc::new(AuxiliaryState {
            calls: std::sync::atomic::AtomicUsize::new(0),
            clock: s.world.app.clock.clone(),
            expire,
        });
        let full = resource_reply::with_input_completed_replay::<WithAuxiliary>(
            &ctx,
            s.owner,
            AuxiliaryInput {
                created: s.created(),
                state: Arc::clone(&state),
            },
        )
        .await?;
        assert!(full.is_none());
        assert_eq!(
            state.calls.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(expire)
        );
        assert_eq!(s.count().await?, before);
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_singleton_auxiliary_read_keeps_first_sql_lifetime_and_does_not_run_without_response_read()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof,
> {
    ::std::boxed::Box::pin(self::singleton_auxiliary_read_keeps_first_sql_lifetime_and_does_not_run_without_response_read())
}
