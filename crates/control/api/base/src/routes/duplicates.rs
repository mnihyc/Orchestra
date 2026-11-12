//! Native configuration duplication. Each family keeps its own ordinary source and creation gates.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use orch_api_contract::duplicates::DuplicateKind;

pub use orch_api_contract::duplicates::DuplicateReceipt;

pub use orch_types::Id;

pub use crate::key_operations::native::effects;

use orch_api_contract::routes::Endpoint;

pub use orch_db::DbError;
orch_api_base::api_family_callback! {
    bind __api_bind_fa1774d3f2, __api_install_fa1774d3f2, __api_callback_fa1774d3f2;
    slot __ApiCallbackfa1774d3f2, __API_CALLBACK_FA1774D3F2, __api_set_fa1774d3f2;
    base orch_api_base::routes::duplicates;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_a8c9718aea, __api_install_a8c9718aea, __api_callback_a8c9718aea;
    slot __ApiCallbacka8c9718aea, __API_CALLBACK_A8C9718AEA, __api_set_a8c9718aea;
    base orch_api_base::routes::duplicates;

    fn receipt(arg0: orch_api_contract::duplicates::DuplicateKind, arg1: orch_types::Id) -> orch_api_contract::duplicates::DuplicateReceipt;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6f65f9e908, __api_install_6f65f9e908, __api_callback_6f65f9e908;
    slot __ApiCallback6f65f9e908, __API_CALLBACK_6F65F9E908, __api_set_6f65f9e908;
    base orch_api_base::routes::duplicates;

    fn destination<'__api>(arg0: &'__api orch_api_contract::duplicates::DuplicateBody, arg1: Option<orch_types::Id>) -> Option<orch_types::Id>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e598983ef2, __api_install_e598983ef2, __api_callback_e598983ef2;
    slot __ApiCallbacke598983ef2, __API_CALLBACK_E598983EF2, __api_set_e598983ef2;
    base orch_api_base::routes::duplicates;

    fn current<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api orch_authz::Caller) -> orch_api_base::middleware::requests::ApiResult<()>;
}

/// Registration and truthful creation identity are common; every native family owns its sources and writer.
pub trait Configuration: effects::Apply<Query = (), Output = Id> {
    const KIND: DuplicateKind;
    const REPLAY: &'static str;
    const MOVING: bool = false;
    fn committed(ctx: &Ctx) {
        ctx.app.engine.kick();
    }
}

pub fn register<F, E>(registry: &mut crate::registry::Registry, endpoint: E)
where
    F: Configuration,
    E: Endpoint<Query = (), Body = F::Body, Reply = DuplicateReceipt> + Copy,
    F::Body: serde::de::DeserializeOwned,
{
    registry.on_with_operation(endpoint, effects::prepare::<F>, copy::<F>);
    replay::<F, E>(registry, endpoint);
}

pub fn replay<F, E>(registry: &mut crate::registry::Registry, endpoint: E)
where
    F: Configuration,
    E: Endpoint<Query = (), Body = F::Body, Reply = DuplicateReceipt>,
    F::Body: serde::de::DeserializeOwned,
{
    registry.creation(endpoint, creation_binding::<F, F::Body>());
}

pub trait CreationIdentity {
    const CREATED_KIND: DuplicateKind;
    const CREATED_REPLAY: &'static str;
}

impl<F: Configuration> CreationIdentity for F {
    const CREATED_KIND: DuplicateKind = F::KIND;
    const CREATED_REPLAY: &'static str = F::REPLAY;
}

pub fn creation_binding<I: CreationIdentity, B>()
-> crate::middleware::idempotency::Creation<(), B, DuplicateReceipt> {
    crate::middleware::idempotency::Creation {
        kind: I::CREATED_REPLAY,
        identity: |value: &DuplicateReceipt| {
            Ok(orch_db::idempotency::creation::Created {
                kind: I::CREATED_REPLAY.into(),
                id: value.id,
                data: serde_json::Value::Null,
            })
        },
        replay: |_ctx, (): (), _body: B, created| {
            Box::pin(async move {
                if created.kind != I::CREATED_REPLAY || !created.data.is_null() {
                    return Err(ApiError::internal("the native duplicate identity changed"));
                }
                Ok(receipt(I::CREATED_KIND, created.id))
            })
        },
    }
}

pub async fn copy<F: Configuration>(
    ctx: Ctx,
    (): (),
    _body: F::Body,
) -> ApiResult<DuplicateReceipt> {
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<F>(&ctx, &mut tx).await?;
    guard.check_deadline(&done.ctx)?;
    if !F::MOVING {
        record_copy(&done.ctx, &mut tx, F::KIND, ctx.id("id")?, done.output).await?;
    }
    guard.check_deadline(&done.ctx)?;
    orch_db::idempotency::creation::observe(
        &mut tx,
        Some(done.ctx.user()?.id()),
        F::REPLAY,
        done.output,
    )
    .await?;
    guard.check_deadline(&done.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    F::committed(&done.ctx);
    let value = receipt(F::KIND, done.output);
    crate::key_operations::publication::receipt(&done.ctx, &value)?;
    Ok(value)
}

orch_api_base::api_family_callback! {
    bind __api_bind_696be1c924, __api_install_696be1c924, __api_callback_696be1c924;
    slot __ApiCallback696be1c924, __API_CALLBACK_696BE1C924, __api_set_696be1c924;
    base orch_api_base::routes::duplicates;

    fn revision<'__api>(arg0: &'__api orch_api_contract::duplicates::DuplicateBody, arg1: i64) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_f983cce395, __api_install_f983cce395, __api_callback_f983cce395;
    slot __ApiCallbackf983cce395, __API_CALLBACK_F983CCE395, __api_set_f983cce395;
    base orch_api_base::routes::duplicates;

    async fn copy_to<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_engine::copy_out::Subject, arg3: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<Option<orch_types::Id>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_cdacc514fb, __api_install_cdacc514fb, __api_callback_cdacc514fb;
    slot __ApiCallbackcdacc514fb, __API_CALLBACK_CDACC514FB, __api_set_cdacc514fb;
    base orch_api_base::routes::duplicates;

    async fn copy_boundary<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_engine::copy_out::Subject, arg3: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<Option<orch_engine::copy_out::Boundary>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_29e0fce0ab, __api_install_29e0fce0ab, __api_callback_29e0fce0ab;
    slot __ApiCallback29e0fce0ab, __API_CALLBACK_29E0FCE0AB, __api_set_29e0fce0ab;
    base orch_api_base::routes::duplicates;

    fn subject(arg0: orch_api_contract::duplicates::DuplicateKind, arg1: orch_types::Id) -> orch_engine::copy_out::Subject;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2078e648bd, __api_install_2078e648bd, __api_callback_2078e648bd;
    slot __ApiCallback2078e648bd, __API_CALLBACK_2078E648BD, __api_set_2078e648bd;
    base orch_api_base::routes::duplicates;

    fn copy_error(arg0: orch_engine::copy_out::Error) -> orch_api_base::middleware::requests::ApiError;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0a665b42ba, __api_install_0a665b42ba, __api_callback_0a665b42ba;
    slot __ApiCallback0a665b42ba, __API_CALLBACK_0A665B42BA, __api_set_0a665b42ba;
    base orch_api_base::routes::duplicates;

    async fn record_copy<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_api_contract::duplicates::DuplicateKind, arg3: orch_types::Id, arg4: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_6d7f4bf1fb, __api_install_6d7f4bf1fb, __api_callback_6d7f4bf1fb;
    slot __ApiCallback6d7f4bf1fb, __API_CALLBACK_6D7F4BF1FB, __api_set_6d7f4bf1fb;
    base orch_api_base::routes::duplicates;

    async fn copy_tags<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_access_contract::permissions::ResourceKind, arg2: orch_types::Id, arg3: Option<orch_types::Id>) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_types::Id>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_de7dea446f, __api_install_de7dea446f, __api_callback_de7dea446f;
    slot __ApiCallbackde7dea446f, __API_CALLBACK_DE7DEA446F, __api_set_de7dea446f;
    base orch_api_base::routes::duplicates;

    fn display_name<'__api>(arg0: &'__api orch_api_contract::duplicates::DuplicateBody, arg1: &'__api str) -> String;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0fea42175e, __api_install_0fea42175e, __api_callback_0fea42175e;
    slot __ApiCallback0fea42175e, __API_CALLBACK_0FEA42175E, __api_set_0fea42175e;
    base orch_api_base::routes::duplicates;

    async fn owns<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_access_contract::decisions::ObjectRef) -> orch_api_base::middleware::requests::ApiResult<bool>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_090d6c0668, __api_install_090d6c0668, __api_callback_090d6c0668;
    slot __ApiCallback090d6c0668, __API_CALLBACK_090D6C0668, __api_set_090d6c0668;
    base orch_api_base::routes::duplicates;

    async fn secret_values<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api str, arg2: orch_types::Id, arg3: &'__api std::collections::BTreeSet<String>) -> orch_api_base::middleware::requests::ApiResult<std::collections::BTreeMap<String, String>>;
}

#[path = "duplicates/moving.rs"]
pub mod moving;
