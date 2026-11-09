//! Real adoption prerequisites stay distinct from the later native creator footprint.

use super::*;

use crate::middleware::authentication;

use crate::places::Who;

use crate::rig::Scene;

use orch_access_contract::key_scope::KeyScope;

use orch_api_contract::adoption_sources::ADOPT_PREPARATION;

use orch_api_contract::adoption_sources::Source;

use orch_api_contract::nodes::ThreadAdopt;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::surface::HttpSurface;

use orch_db::queries::identity;

use orch_engine::adoption::Adoption;

use orch_engine::adoption::operation as adoption;

use orch_types::new_id;

use serde_json::Value;

use serde_json::json;

use std::sync::atomic::AtomicUsize;
pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy)]
enum FactCase {
    Complete,
    Missing(Source),
    Duplicate,
    WrongCardinality,
    Undeclared,
}

struct Input {
    adoption: Adoption,
    facts: FactCase,
    requires: AtomicUsize,
    fetches: AtomicUsize,
    holds: AtomicUsize,
}

struct Fetched(Arc<Input>);

struct Entry;

impl Effect for Entry {
    type Source = Source;
    type Query = ();
    type Body = Arc<Input>;
    type Value = Arc<Input>;
    type Output = ();

    fn declaration(_: &Arc<Input>) -> &'static Declaration<Source> {
        &ADOPT_PREPARATION
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        input: Arc<Input>,
    ) -> SourceFuture<'a, Arc<Input>, Source> {
        Box::pin(async move {
            if initial_frame(ctx)?.is_some() {
                let fetched = initial_value::<Self>(ctx)?;
                assert!(Arc::ptr_eq(&fetched.0, &input));
            }
            let mut facts = adoption::initial_facts(conn, &input.adoption)
                .await
                .map_err(ApiError::engine)?;
            match input.facts {
                FactCase::Complete => {}
                FactCase::Missing(role) => facts.retain(|fact| fact.source != role),
                FactCase::Duplicate => facts.push(facts[1].clone()),
                FactCase::WrongCardinality => {
                    facts[0].cardinality =
                        orch_api_contract::operation_sources::Cardinality::Collection;
                }
                FactCase::Undeclared => {
                    facts.push(Facts::collection(Source::PreviewContent, vec![]))
                }
            }
            Ok((input, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            input.requires.fetch_add(1, Ordering::SeqCst);
            if principal.second_step_hold().is_some() {
                input.holds.fetch_add(1, Ordering::SeqCst);
            }
            let caller = principal.caller();
            assert!(caller.admitted_operation().is_none());
            assert!(caller.admitted_required_roles().is_none());
            assert!(caller.admitted_empty_facets().is_none());
            assert!(orch_authz::CapturedOperation::capture(caller).is_err());
            adoption::require_initial(conn, principal, &input.adoption)
                .await
                .map_err(ApiError::internal)
        })
    }
}

impl Initial for Entry {
    type Input = Arc<Input>;
    type Fetched = Fetched;

    fn declaration(_: &Arc<Input>) -> &'static Declaration<Source> {
        &ADOPT_PREPARATION
    }
    fn authorize<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        input: Arc<Input>,
    ) -> SourceFuture<'a, Arc<Input>, Source> {
        Box::pin(async move {
            let facts = adoption::initial_facts(conn, &input.adoption)
                .await
                .map_err(ApiError::engine)?;
            Ok((input, facts))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            adoption::require_initial(conn, principal, &input.adoption)
                .await
                .map_err(ApiError::internal)
        })
    }
    fn fetch<'a>(
        _: &'a Ctx,
        _: &'a Caller,
        input: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            input.fetches.fetch_add(1, Ordering::SeqCst);
            Ok(Fetched(input))
        })
    }
}

fn input(s: &Scene, facts: FactCase) -> Arc<Input> {
    Arc::new(Input {
        adoption: Adoption {
            agent_access: None,
            topic: s.tree.topic.base.id,
            provider: s.on.provider,
            agent: s.on.agent,
            native_session_ref: "actual-prerequisite-entry".into(),
            environment: None,
            name: None,
        },
        facts,
        requires: AtomicUsize::new(0),
        fetches: AtomicUsize::new(0),
        holds: AtomicUsize::new(0),
    })
}

fn scope(s: &Scene, binding: bool) -> TestResult<Value> {
    let value = json!({"rules":[
        {"target":{"kind":"topic","id":s.tree.topic.base.id,"reach":"self"},
            "resources":["thread"],"allow":[{"operation":"ThreadAdopt","actions":["write"]}]},
        {"target":{"kind":"provider","id":s.on.provider,"reach":"self"},
            "resources":["provider"],"allow":[{"operation":"ThreadAdopt","actions":["read","write"]}]},
        {"target":{"kind":"agent","id":s.on.agent,"reach":"self"},
            "resources":["agent"],"allow":[{"operation":"ThreadAdopt","actions":if binding {vec!["write"]} else {vec!["read"]}}]}
    ]});
    let scope: KeyScope = serde_json::from_value(value.clone())?;
    assert!(scope.validate().is_empty());
    Ok(value)
}

async fn context(s: &Scene, who: &Who, session: bool) -> TestResult<(Ctx, Caller, Option<Id>)> {
    let mut conn = s.rig.world.conn().await;
    let (bearer, token, surface) = if session {
        (who.token.clone(), None, HttpSurface::Browser)
    } else {
        let bearer = authentication::new_token(authentication::API_TOKEN_PREFIX);
        let id = new_id();
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id,
                user_id: who.id(),
                name: "Declared native prerequisites",
                token_hash: &authentication::token_hash(&bearer),
                scopes: &[],
                node_id: None,
                at: s.rig.world.now(),
                expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
            },
            Some(&scope(s, true)?),
        )
        .await?;
        (bearer, Some(id), HttpSurface::Key)
    };
    drop(conn);
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {bearer}").parse()?,
    );
    let user =
        authentication::authenticate_on(surface, &s.rig.world.app.app, &headers, s.rig.world.now())
            .await?;
    let mut ctx = Ctx::for_tool(
        s.rig.world.app.app.clone(),
        user,
        ThreadAdopt::ROUTE,
        s.tree.topic.base.id,
    );
    ctx.surface = Some(surface);
    ctx.headers = headers;
    let caller = authorization::caller_of(ctx.user()?)?;
    Ok((ctx, caller, token))
}

async fn assert_no_creation(s: &Scene) -> TestResult {
    let threads: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread'")
        .fetch_one(s.rig.db.pool())
        .await?;
    assert_eq!(threads, 0);
    assert_eq!(s.rig.commands.issued_total(), 0);
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn declared_adoption_entry_checks_real_prerequisites_without_admitting_future_creator_roles()
-> TestResult {
    let s = Scene::new().await;
    for session in [false, true] {
        let (ctx, caller, _) = context(&s, &s.ada, session).await?;
        let input = input(&s, FactCase::Complete);
        initial::<Entry>(&ctx, &caller, (), Arc::clone(&input)).await?;
        let mut conn = s.rig.world.conn().await;
        for _ in 0..2 {
            let plan =
                prepare_entry::<Entry>(&ctx, &mut conn, &caller, (), Arc::clone(&input)).await?;
            assert_eq!(plan.variant().operation, "ThreadAdopt");
            assert_eq!(plan.shared().known_required_facets().len(), 4);
            assert!(plan.shared().required_facets().is_err());
            assert!(plan.clone().admit(&caller).is_err());
            let prefix = plan.adoption_creation_prefix()?;
            assert!(prefix.is_topic_creation_prefix());
            assert!(prefix.admit(&caller).is_err());
            assert!(!orch_db::queries::transaction_guard::in_transaction(
                &mut conn
            ));
        }
        assert_eq!(input.requires.load(Ordering::SeqCst), 2);
        assert_eq!(input.fetches.load(Ordering::SeqCst), 1);
        let Err(error) = prepare::<Entry>(&ctx, &mut conn, &caller, (), Arc::clone(&input)).await
        else {
            return Err("the incomplete creator entered the full effect frame".into());
        };
        assert_eq!(
            error.code(),
            orch_api_contract::common::ErrorCode::AuthzDenied
        );
        assert_eq!(
            input.requires.load(Ordering::SeqCst),
            2,
            "the full effect gate still refuses before ordinary authorization"
        );
    }
    assert_no_creation(&s).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_declared_adoption_entry_checks_real_prerequisites_without_admitting_future_creator_roles()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::key_operations::native::effects::entry_tests::TestResult,
> {
    ::std::boxed::Box::pin(self::declared_adoption_entry_checks_real_prerequisites_without_admitting_future_creator_roles())
}

#[cfg(any(test, feature = "test-support"))]
async fn declared_entry_rejects_each_missing_duplicate_mistyped_or_undeclared_source_before_require()
-> TestResult {
    let s = Scene::new().await;
    for case in [
        FactCase::Missing(Source::Destination),
        FactCase::Missing(Source::Session),
        FactCase::Missing(Source::Binding),
        FactCase::Duplicate,
        FactCase::WrongCardinality,
        FactCase::Undeclared,
    ] {
        let (ctx, caller, _) = context(&s, &s.ada, false).await?;
        let input = input(&s, case);
        initial::<Entry>(&ctx, &caller, (), Arc::clone(&input)).await?;
        let mut conn = s.rig.world.conn().await;
        let Err(error) =
            prepare_entry::<Entry>(&ctx, &mut conn, &caller, (), Arc::clone(&input)).await
        else {
            return Err("an incomplete or invalid declared source entered preparation".into());
        };
        assert_eq!(error.code(), orch_api_contract::common::ErrorCode::Internal);
        assert_eq!(input.requires.load(Ordering::SeqCst), 0);
        assert_eq!(input.fetches.load(Ordering::SeqCst), 1);
        assert!(!orch_db::queries::transaction_guard::in_transaction(
            &mut conn
        ));
    }
    assert_no_creation(&s).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_declared_entry_rejects_each_missing_duplicate_mistyped_or_undeclared_source_before_require()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::key_operations::native::effects::entry_tests::TestResult,
> {
    ::std::boxed::Box::pin(self::declared_entry_rejects_each_missing_duplicate_mistyped_or_undeclared_source_before_require())
}

#[cfg(any(test, feature = "test-support"))]
async fn declared_entry_keeps_the_initial_origin_current_scope_and_actual_raw_lifetime()
-> TestResult {
    let s = Scene::new().await;
    let (ctx, caller, token) = context(&s, &s.ada, false).await?;
    let input = input(&s, FactCase::Complete);
    initial::<Entry>(&ctx, &caller, (), Arc::clone(&input)).await?;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token)
        .bind(scope(&s, false)?)
        .execute(s.rig.db.pool())
        .await?;
    let mut conn = s.rig.world.conn().await;
    assert!(
        prepare_entry::<Entry>(&ctx, &mut conn, &caller, (), Arc::clone(&input))
            .await
            .is_err()
    );
    assert_eq!(input.requires.load(Ordering::SeqCst), 0);
    drop(conn);

    let (ctx, caller, _) = context(&s, &s.ada, true).await?;
    let browser_input = self::input(&s, FactCase::Complete);
    initial::<Entry>(&ctx, &caller, (), Arc::clone(&browser_input)).await?;
    let end: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT expires_at FROM access_tokens WHERE token_hash=$1")
            .bind(authentication::token_hash(&s.ada.token))
            .fetch_one(s.rig.db.pool())
            .await?;
    s.rig
        .world
        .advance(end - s.rig.world.now() + chrono::Duration::milliseconds(1));
    let mut conn = s.rig.world.conn().await;
    assert!(
        prepare_entry::<Entry>(&ctx, &mut conn, &caller, (), Arc::clone(&browser_input))
            .await
            .is_err()
    );
    assert_eq!(browser_input.requires.load(Ordering::SeqCst), 0);
    assert_eq!(browser_input.fetches.load(Ordering::SeqCst), 1);
    assert_no_creation(&s).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_declared_entry_keeps_the_initial_origin_current_scope_and_actual_raw_lifetime()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::key_operations::native::effects::entry_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::declared_entry_keeps_the_initial_origin_current_scope_and_actual_raw_lifetime(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn declared_entry_never_uses_scope_in_place_of_ordinary_authority_or_a_second_step_hold()
-> TestResult {
    let s = Scene::new().await;
    let (ctx, caller, _) = context(&s, &s.dan, false).await?;
    let input = input(&s, FactCase::Complete);
    let mut conn = s.rig.world.conn().await;
    assert!(
        prepare_entry::<Entry>(&ctx, &mut conn, &caller, (), Arc::clone(&input))
            .await
            .is_err()
    );
    assert_eq!(
        input.requires.load(Ordering::SeqCst),
        1,
        "the real provider/agent Use gate must run after scope admission"
    );
    drop(conn);

    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(s.tree.org.base.id),
            "auth.second_step",
            json!("required"),
        )
        .await;
    let (ctx, caller, _) = context(&s, &s.ada, true).await?;
    let held_input = self::input(&s, FactCase::Complete);
    let mut conn = s.rig.world.conn().await;
    assert!(
        prepare_entry::<Entry>(&ctx, &mut conn, &caller, (), Arc::clone(&held_input))
            .await
            .is_err()
    );
    assert_eq!(held_input.requires.load(Ordering::SeqCst), 1);
    assert_eq!(held_input.holds.load(Ordering::SeqCst), 1);
    assert_no_creation(&s).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_declared_entry_never_uses_scope_in_place_of_ordinary_authority_or_a_second_step_hold()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::key_operations::native::effects::entry_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::declared_entry_never_uses_scope_in_place_of_ordinary_authority_or_a_second_step_hold(
        ),
    )
}
