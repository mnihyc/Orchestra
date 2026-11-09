//! Existing metadata writers prepare their complete native read-only prefix in
//! the final guarded operation, then consume those exact operands before SQL.

use super::Placed;

use super::color;

use super::placed;

use super::readme_choice;

use super::shown;

use crate::key_operations::admission;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeTagSetBody;

use orch_api_contract::nodes::NodeUpdateBody;

use orch_api_contract::nodes::Topic;

use orch_api_contract::nodes::TopicUpdateBody;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;
#[derive(Clone)]
pub(super) struct Fields {
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<Option<i16>>,
    pub own: Option<Option<String>>,
}

pub(super) fn fields(body: &NodeUpdateBody) -> ApiResult<Fields> {
    Ok(Fields {
        color: color::checked(body.color)?,
        own: color::own(body.color, body.custom_color.as_ref().map(Option::as_deref))?,
        name: body
            .name
            .as_deref()
            .map(|name| super::clean_name("name", name))
            .transpose()?,
        description: body
            .description
            .as_deref()
            .map(|text| super::clean_description("description", text))
            .transpose()?,
    })
}

#[derive(Clone)]
pub(super) struct Update {
    pub fields: Fields,
    pub place: Placed,
    pub kind: NodeKind,
    pub readme: Option<Option<Id>>,
    pub before: Node,
}

impl Update {
    pub fn noop(&self) -> bool {
        self.fields.name.is_none()
            && self.fields.description.is_none()
            && self.fields.color.is_none()
            && self.fields.own.is_none()
            && self.readme.is_none()
    }
}

#[derive(Clone)]
pub(super) struct TopicChange {
    pub place: Placed,
    pub before: Topic,
}

#[derive(Clone)]
pub(super) struct Tags {
    pub place: Placed,
    pub before: Node,
}

/// Preserve the declared ordinary door before a native prefix can produce metadata/errors.
/// The common guard repeats this same declared permission at its current checkpoint.
async fn principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &crate::registry::OperationPlan,
) -> ApiResult<Principal> {
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let orch_api_contract::routes::Access::Permission(permission) = ctx.route.access else {
        return Err(ApiError::internal(
            "metadata lost its declared ordinary permission",
        ));
    };
    authorization::require_on_path_in(ctx, conn, &principal, permission).await?;
    Ok(principal)
}

pub(super) fn update<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: NodeUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        // Preserve input validation before object/native prefix discovery.
        let fields = fields(&body)?;
        let plan = admission::prepare_at_path(ctx, conn)
            .await?
            .ok_or_else(|| {
                ApiError::internal("node metadata needs its existing complete path plan")
            })?;
        let principal = principal(ctx, conn, caller, &plan).await?;
        let id = ctx.id("id")?;
        let (place, kind) = placed(conn, id).await?;
        let readme = readme_choice(conn, id, body.readme_document_id).await?;
        let before = shown(ctx, conn, &principal, id).await?;
        Ok(plan.with_data(Update {
            fields,
            place,
            kind,
            readme,
            before,
        }))
    })
}

pub(super) fn topic<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: TopicUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let plan = admission::prepare_at_path(ctx, conn)
            .await?
            .ok_or_else(|| {
                ApiError::internal("topic metadata needs its existing complete path plan")
            })?;
        let principal = principal(ctx, conn, caller, &plan).await?;
        let prepared = super::tree::prepare_topic(ctx, conn, &principal, &body).await?;
        Ok(plan.with_data(prepared))
    })
}

pub(super) fn tags<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: NodeTagSetBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let plan = admission::prepare_at_path(ctx, conn)
            .await?
            .ok_or_else(|| {
                ApiError::internal("node tags need their existing complete path plan")
            })?;
        let principal = principal(ctx, conn, caller, &plan).await?;
        let prepared = super::tree::prepare_tags(ctx, conn, &principal, &body).await?;
        Ok(plan.with_data(prepared))
    })
}
