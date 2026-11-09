//! The actual raw checkpoint runs on the supplied connection and cannot commit a preparation trial.
use super::*;
use orch_authz::CapturedOperation;
use orch_engine::operation_scope::continuing as native_frame;
use orch_types::{Hash, Time};

struct UploadInput {
    org: Id,
    blob: Hash,
}
struct UploadSource;

async fn facts(conn: &mut Conn, caller: &Caller, org: Id) -> ApiResult<Vec<Facts<Source>>> {
    let target = crate::key_operations::targets::node_identity(conn, org).await?;
    let mut facts = upload_facts(caller.user());
    facts.push(Facts::collection(
        Source::Destinations,
        vec![(target.object.kind, vec![target])],
    ));
    Ok(facts)
}

fn native_error(error: ApiError) -> native_frame::Error {
    native_frame::Error::Native(orch_engine::EngineError::BeforeEffect(Box::new(error)))
}
fn api_error(error: native_frame::Error) -> ApiError {
    match error {
        native_frame::Error::Refused => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the actual preparation source was refused",
        ),
        native_frame::Error::Binding(error) => admission::binding_error(error),
        native_frame::Error::Native(error) => ApiError::engine(error),
    }
}

impl native_frame::Source for UploadSource {
    type Role = Source;
    type Input = UploadInput;
    type Value = Hash;
    const ROUTE: orch_api_contract::routes::Route = NodeImportCreate::ROUTE;
    const DECLARATION: &'static Declaration<Source> = &ROWS[0];
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Self::Input,
        _: Time,
    ) -> native_frame::PreparedFuture<'a, Hash, Source> {
        Box::pin(async move {
            Ok((
                input.blob,
                facts(conn, caller, input.org).await.map_err(native_error)?,
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        value: &'a Hash,
    ) -> BoxFuture<'a, Result<(), native_frame::Error>> {
        Box::pin(async move {
            crate::attachments::require(conn, principal, value)
                .await
                .map_err(native_error)?;
            authorization::require(
                conn,
                principal,
                input.org,
                authorization::VIEW,
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await
            .map_err(native_error)
        })
    }
    fn locks<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Input,
        _: &'a Hash,
    ) -> BoxFuture<'a, Result<(), native_frame::Error>> {
        Box::pin(async { Ok(()) })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Input,
        old: &'a Hash,
        current: &'a Hash,
    ) -> BoxFuture<'a, Result<bool, native_frame::Error>> {
        Box::pin(async move { Ok(old == current) })
    }
}

struct Checkpointed;
struct Observed {
    backend: i32,
}
impl Effect for Checkpointed {
    type Source = Source;
    type Query = ();
    type Body = Arc<Input>;
    type Value = Observed;
    type Output = ();
    fn declaration(_: &Observed) -> &'static Declaration<Source> {
        &ROWS[0]
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Arc<Input>,
    ) -> super::super::super::SourceFuture<'a, Observed, Source> {
        Box::pin(async move {
            // A real rollbackable metadata trial must not escape a preliminary resolver.
            let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            sqlx::query("INSERT INTO effect_prepare_trial VALUES(1)")
                .execute(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            let org = ctx.id("id")?;
            let facts = facts(conn, caller, org).await?;
            let plan = bind_required(ctx, &ROWS[0], &facts)?;
            let accepted = plan.admit(caller)?.caller;
            let captured = CapturedOperation::capture(&accepted).map_err(ApiError::internal)?;
            let origin =
                super::super::super::http_origin::capture_operation(ctx, conn, &captured).await?;
            origin
                .current(conn, ctx.app.clock.as_ref())
                .await
                .map_err(api_error)?
                .check(ctx.app.clock.as_ref())
                .map_err(api_error)?;
            let frame = origin
                .scope(Box::pin(native_frame::checkpoint::<UploadSource>(
                    conn,
                    ctx.app.clock.as_ref(),
                    &origin,
                    UploadInput {
                        org,
                        blob: body.request.source.files[0].blob,
                    },
                )))
                .await
                .map_err(api_error)?;
            frame
                .check_deadline(ctx.app.clock.as_ref())
                .map_err(api_error)?;
            assert_eq!(frame.principal().user(), caller.user());
            Ok((Observed { backend }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Observed,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

async fn trial_table(conn: &mut Conn) -> TestResult {
    sqlx::query("CREATE TEMP TABLE effect_prepare_trial(value integer NOT NULL)")
        .execute(conn)
        .await?;
    Ok(())
}
async fn trials(conn: &mut Conn) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM effect_prepare_trial")
            .fetch_one(conn)
            .await?,
    )
}

#[tokio::test]
async fn preliminary_effect_preparation_runs_its_real_http_checkpoint_on_the_same_connection_without_committing_trials()
-> TestResult {
    for session in [false, true] {
        let s = Fixture::new(session).await?;
        let caller = authorization::caller_of(s.ctx.user()?)?;
        let mut conn = s.world.conn().await;
        trial_table(&mut conn).await?;
        let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await?;
        assert!(!orch_db::queries::transaction_guard::in_transaction(
            &mut conn
        ));
        for _ in 0..2 {
            let plan =
                prepare::<Checkpointed>(&s.ctx, &mut conn, &caller, (), Arc::clone(&s.input))
                    .await?;
            assert_eq!(plan.variant().operation, ROWS[0].operation);
            let admitted = plan.admit(&caller)?;
            let operands = admitted.data.get::<Operands<Checkpointed>>()?;
            assert_eq!(operands.value.backend, backend);
            assert_eq!(trials(&mut conn).await?, 0);
            assert!(!orch_db::queries::transaction_guard::in_transaction(
                &mut conn
            ));
        }
        let imports: i64 = sqlx::query_scalar("SELECT count(*) FROM imports")
            .fetch_one(&mut *conn)
            .await?;
        assert_eq!(imports, 0);
    }
    Ok(())
}

#[tokio::test]
async fn effect_preparation_keeps_the_callers_existing_transaction_and_rolls_back_only_with_its_owner()
-> TestResult {
    let s = Fixture::new(false).await?;
    let caller = authorization::caller_of(s.ctx.user()?)?;
    let mut conn = s.world.conn().await;
    trial_table(&mut conn).await?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let plan = prepare::<Checkpointed>(&s.ctx, &mut tx, &caller, (), Arc::clone(&s.input)).await?;
    assert_eq!(plan.variant().operation, ROWS[0].operation);
    assert!(orch_db::queries::transaction_guard::in_transaction(&mut tx));
    assert_eq!(
        trials(&mut tx).await?,
        1,
        "the owner still holds its exact uncommitted trial"
    );
    tx.rollback().await?;
    assert_eq!(trials(&mut conn).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_refused_preliminary_checkpoint_leaves_no_trial_and_does_not_erase_current_scope_or_raw_lifetime()
-> TestResult {
    for raw in [false, true] {
        let s = Fixture::new(raw).await?;
        if raw {
            s.world.advance(chrono::Duration::seconds(6));
        } else {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(s.token)
                .bind(serde_json::to_value(scope(s.owner, s.org, false)?)?)
                .execute(&s.world.db.pool)
                .await?;
        }
        let caller = authorization::caller_of(s.ctx.user()?)?;
        let mut conn = s.world.conn().await;
        trial_table(&mut conn).await?;
        assert!(
            prepare::<Checkpointed>(&s.ctx, &mut conn, &caller, (), Arc::clone(&s.input))
                .await
                .is_err()
        );
        assert_eq!(trials(&mut conn).await?, 0);
        assert!(!orch_db::queries::transaction_guard::in_transaction(
            &mut conn
        ));
        let answer: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&mut *conn).await?;
        assert_eq!(
            answer, 1,
            "the same connection remains usable after refused preparation"
        );
    }
    Ok(())
}
