//! Native contextual projections of the existing resource read operations.
//!
//! A composition supplies its genuine operation Ctx and selected place. The resource
//! keeps its real home; no URL, model assertion or global Manage becomes a work grant.

pub use crate::key_operations::native;

use crate::key_operations::publication;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use crate::registry::OperationPlan;

pub use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::permissions::Options;

pub use orch_access_contract::permissions::Permission;

pub use orch_api_contract::nodes::NodeRef;

pub use orch_authz::Caller;

pub use orch_authz::CapturedOperation;

pub use orch_authz::resources;

pub use orch_db::Conn;

pub use orch_types::Id;

use serde::Serialize;
/// Immutable native selector. These are candidates, never a claimed resource parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub node: Id,
    pub after: Option<Id>,
    pub limit: Option<u32>,
}

/// An entry's options in their canonical stored form.
pub fn options_json<S: serde::Serializer>(
    options: &Options,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(&options.to_json(), serializer)
}

/// Exactly the intrinsic reference and the selected work grant, with no global resource details.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resource {
    pub id: Id,
    pub kind: String,
    pub name: String,
    pub disabled: bool,
    pub permission: Permission,
    #[serde(serialize_with = "options_json")]
    pub options: Options,
}

/// A limited native composition result; it never pretends to be a full Provider or Agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Projection {
    pub projection: &'static str,
    pub context: NodeRef,
    pub items: Vec<Resource>,
    pub next_cursor: Option<String>,
}

/// Actual native context and resource identities for the consuming composition's root check.
/// This is not serializable authority, and never changes a resource's true ancestry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFacts {
    pub context: KeyResolvedTarget,
    pub resources: Vec<KeyResolvedTarget>,
}

#[derive(Clone)]
pub struct Prepared {
    pub response: OperationPlan,
    pub origin: CapturedOperation,
    pub request: Request,
    pub grants: Vec<resources::context::Grant>,
    pub facts: ContextFacts,
    pub projection: Projection,
}

orch_api_base::api_family_callback! {
    bind __api_bind_4961d39df5, __api_install_4961d39df5, __api_callback_4961d39df5;
    slot __ApiCallback4961d39df5, __API_CALLBACK_4961D39DF5, __api_set_4961d39df5;
    base orch_api_base::routes::resources::context;

    fn family<'__api>(arg0: &'__api str) -> orch_api_base::middleware::requests::ApiResult<(orch_access_contract::permissions::ResourceKind, bool)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_35df7b5ec1, __api_install_35df7b5ec1, __api_callback_35df7b5ec1;
    slot __ApiCallback35df7b5ec1, __API_CALLBACK_35DF7B5EC1, __api_set_35df7b5ec1;
    base orch_api_base::routes::resources::context;

    fn facts<'__api>(arg0: &'__api orch_api_base::key_operations::admission::Prepared) -> orch_api_base::middleware::requests::ApiResult<Option<orch_api_base::routes::resources::context::ContextFacts>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_19bde9b549, __api_install_19bde9b549, __api_callback_19bde9b549;
    slot __ApiCallback19bde9b549, __API_CALLBACK_19BDE9B549, __api_set_19bde9b549;
    base orch_api_base::routes::resources::context;

    async fn bind<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_types::Id, arg3: &'__api [orch_authz::resources::context::Grant]) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::key_operations::admission::Prepared, orch_api_base::routes::resources::context::ContextFacts)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_75a6668c1c, __api_install_75a6668c1c, __api_callback_75a6668c1c;
    slot __ApiCallback75a6668c1c, __API_CALLBACK_75A6668C1C, __api_set_75a6668c1c;
    base orch_api_base::routes::resources::context;

    async fn node_ref<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::nodes::NodeRef>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_0f877a22b0, __api_install_0f877a22b0, __api_callback_0f877a22b0;
    slot __ApiCallback0f877a22b0, __API_CALLBACK_0F877A22B0, __api_set_0f877a22b0;
    base orch_api_base::routes::resources::context;

    async fn grants_at<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_authz::resources::context::Grant>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b57f3f3ce1, __api_install_b57f3f3ce1, __api_callback_b57f3f3ce1;
    slot __ApiCallbackb57f3f3ce1, __API_CALLBACK_B57F3F3CE1, __api_set_b57f3f3ce1;
    base orch_api_base::routes::resources::context;

    async fn prepare<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: orch_api_base::routes::resources::context::Request) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::resources::context::Prepared>;
}

impl Prepared {
    /// One business plan for the ordinary read; compositions check these actual source facts.
    pub fn plan(&self) -> &OperationPlan {
        &self.response
    }

    pub async fn current(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
    ) -> ApiResult<OperationPlan> {
        if !self.response.matches_route(&ctx.route) {
            return Err(refused());
        }
        let (_, single) = family(ctx.route.name)?;
        if single
            && self
                .grants
                .first()
                .is_none_or(|grant| ctx.id("id").ok() != Some(grant.resource.id))
        {
            return Err(refused());
        }
        let known = OperationPlan::new(ctx.route, ctx.route.name)?
            .with_origin(ctx, conn, caller, &self.origin)
            .await?;
        let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
        let ids = self
            .grants
            .iter()
            .map(|grant| grant.resource.id)
            .collect::<Vec<_>>();
        let mut grants = grants_at(conn, &principal, self.request.node, &ids).await?;
        grants.sort_by_key(|grant| grant.resource.id);
        let node = node_ref(conn, &principal, self.request.node).await?;
        if grants != self.grants || node != self.projection.context {
            return Err(refused());
        }
        let (response, facts) = bind(ctx, conn, self.request.node, &grants).await?;
        if facts != self.facts {
            return Err(refused());
        }
        response
            .with_origin(ctx, conn, principal.caller(), &self.origin)
            .await
    }

    /// Render only this frozen native projection under the same operation's current Read.
    /// Later publication repeats the source check, never selection of another list window.
    pub async fn read(&self, ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<Projection> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(ApiError::internal(
                "a native contextual projection needs its own snapshot",
            ));
        }
        let response = self.current(ctx, conn, caller).await?;
        let reader = response.preparation_principal(ctx, conn, caller).await?;
        let data = self.projection.clone();
        let shown = response
            .read_operation(ctx, conn, reader.caller(), move |_conn, _reader| {
                Box::pin(async move { Ok(data) })
            })
            .await?
            .ok_or_else(refused)?;
        publication::retain(
            ctx,
            reader.caller(),
            self.clone(),
            |ctx, conn, principal, saved| {
                Box::pin(async move {
                    let response = saved.current(ctx, conn, principal.caller()).await?;
                    let reader = response
                        .preparation_principal(ctx, conn, principal.caller())
                        .await?;
                    response
                        .read_operation(ctx, conn, reader.caller(), |_conn, _reader| {
                            Box::pin(async { Ok(()) })
                        })
                        .await?
                        .ok_or_else(refused)
                })
            },
        )?;
        Ok(shown)
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_e0bd87e4b3, __api_install_e0bd87e4b3, __api_callback_e0bd87e4b3;
    slot __ApiCallbacke0bd87e4b3, __API_CALLBACK_E0BD87E4B3, __api_set_e0bd87e4b3;
    base orch_api_base::routes::resources::context;

    fn refused() -> orch_api_base::middleware::requests::ApiError;
}
