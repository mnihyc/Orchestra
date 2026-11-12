//! Actual movement operands and independent current authority of the stored review.

pub use super::*;

use crate::key_operations::native::effects::Effect;

use crate::key_operations::native::effects::FirstEffect;

pub use crate::key_operations::native;

pub use futures::future::BoxFuture;

pub use orch_access_contract::permissions::ObjectKind;

pub use orch_api_contract::operation_sources::Declaration;

pub use orch_api_contract::thread_move_sources as sources;

pub use orch_api_contract::thread_move_sources::Source;

pub use orch_authz::Caller;

use orch_authz::CapturedOperation;

use serde::Deserialize;

use serde::Serialize;
#[derive(Clone)]
pub struct Base {
    pub row: threads::ThreadRow,
    pub lanes: Vec<threads::ThreadRow>,
    pub target: Id,
    pub target_kind: ObjectKind,
    pub required: Vec<native::Facts<Source>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Association {
    pub id: Id,
    pub topic: Id,
    pub parent: Option<Id>,
    pub side_of: Option<Id>,
    pub environment: Option<Id>,
    pub segment: Option<Id>,
    pub agent: Option<Id>,
    pub provider: Option<Id>,
}

impl Association {
    pub fn of(row: &threads::ThreadRow) -> Self {
        Self {
            id: row.node_id,
            topic: row.topic_id,
            parent: row.parent_id,
            side_of: row.side_of,
            environment: row.environment_id,
            segment: row.current_segment_id,
            agent: row.agent_id,
            provider: row.provider_id,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReviewSources {
    pub main: Id,
    pub target: Id,
    pub from: Id,
    pub lanes: Vec<Association>,
    pub content: Vec<(ObjectKind, Id)>,
}

impl ReviewSources {
    pub fn preserves(&self, base: &Base) -> bool {
        self.main == base.row.node_id
            && self.target == base.target
            && self.from == base.row.topic_id
            && self.lanes == base.lanes.iter().map(Association::of).collect::<Vec<_>>()
    }
    pub fn of(base: &Base, mut content: Vec<(ObjectKind, Id)>) -> Self {
        content.extend([
            (ObjectKind::Topic, base.row.topic_id),
            (base.target_kind, base.target),
        ]);
        if let Some(environment) = base.row.environment_id {
            content.push((ObjectKind::Environment, environment));
        }
        content.sort_unstable();
        content.dedup();
        Self {
            main: base.row.node_id,
            target: base.target,
            from: base.row.topic_id,
            lanes: base.lanes.iter().map(Association::of).collect(),
            content,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Origin {
    pub operation: CapturedOperation,
    pub session: Option<Id>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Kept {
    pub review: ThreadMovePlan,
    pub fingerprint: String,
    #[serde(default)]
    pub sources: Option<ReviewSources>,
    #[serde(default)]
    pub origin: Option<Origin>,
}

pub struct Confirmation {
    pub body: ThreadMoveBody,
    pub kept: Kept,
    pub until: Time,
}

pub struct Prepared {
    pub base: Base,
    pub confirmation: Option<Confirmation>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_df48d8cc75, __api_install_df48d8cc75, __api_callback_df48d8cc75;
    slot __ApiCallbackdf48d8cc75, __API_CALLBACK_DF48D8CC75, __api_set_df48d8cc75;
    base orch_api_base::routes::nodes::moves::thread::operation;

    async fn require<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_api_base::routes::nodes::moves::thread::operation::Base) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_01f987e298, __api_install_01f987e298, __api_callback_01f987e298;
    slot __ApiCallback01f987e298, __API_CALLBACK_01F987E298, __api_set_01f987e298;
    base orch_api_base::routes::nodes::moves::thread::operation;

    async fn prepare<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: orch_types::Id, arg4: Option<orch_api_contract::thread_move::ThreadMoveBody>) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::routes::nodes::moves::thread::operation::Prepared, Vec<orch_engine::operation_scope::sources::Facts<orch_api_contract::thread_move_sources::Source>>)>;
}

pub struct Preview;

pub struct Move;

impl Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = ThreadMovePreviewBody;
    type Value = Prepared;
    type Output = super::Reviewed;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        &sources::PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadMovePreviewBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, body.parent_id, None))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, &value.base))
    }
}

impl Effect for Move {
    type Source = Source;
    type Query = ();
    type Body = ThreadMoveBody;
    type Value = Prepared;
    type Output = super::MovedThread;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        &sources::MOVE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadMoveBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, body.parent_id, Some(body)))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, &value.base))
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_8039c90d48, __api_install_8039c90d48, __api_callback_8039c90d48;
    slot __ApiCallback8039c90d48, __API_CALLBACK_8039C90D48, __api_set_8039c90d48;
    base orch_api_base::routes::nodes::moves::thread::operation;

    fn refused() -> orch_api_base::middleware::requests::ApiError;
}

pub struct OriginalGate {
    // Absence is limited to a pre-upgrade review with neither provenance field.
    // It grants no original authority; the real native expiry still vetoes work.
    pub caller: Option<Caller>,
    pub session_until: Option<Time>,
    pub plan_until: Time,
}

impl OriginalGate {
    pub fn check(&self, ctx: &Ctx) -> ApiResult<()> {
        if self.plan_until <= ctx.app.clock.time() {
            return Err(ApiError::new(
                ErrorCode::PreflightExpired,
                "the thread move review expired; read it again",
            ));
        }
        if self
            .caller
            .as_ref()
            .is_some_and(|caller| !caller.key_deadline_allows(ctx.app.clock.as_ref()))
            || self
                .session_until
                .is_some_and(|end| end <= ctx.app.clock.time())
        {
            return Err(refused());
        }
        Ok(())
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_8ce72302f8, __api_install_8ce72302f8, __api_callback_8ce72302f8;
    slot __ApiCallback8ce72302f8, __API_CALLBACK_8CE72302F8, __api_set_8ce72302f8;
    base orch_api_base::routes::nodes::moves::thread::operation;

    async fn original<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_api_base::routes::nodes::moves::thread::operation::Confirmation) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::nodes::moves::thread::operation::OriginalGate>;
}

pub fn check_first(
    ctx: &Ctx,
    first: &mut Option<FirstEffect<'_>>,
    original: Option<&OriginalGate>,
) -> Result<(), orch_engine::EngineError> {
    if let Some(original) = original {
        original
            .check(ctx)
            .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))?;
    }
    if let Some(first) = first.take() {
        first
            .check()
            .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))?;
    }
    Ok(())
}
