//! Native MCP preset preparation, one insert/sealing/page/audit effect and completed projection.

use super::*;
use crate::key_operations::native::{self, effects};
use crate::middleware::transaction_guard;
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::{creation_home, projection::single};
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::mcp_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::resources::{McpServerCreateReceipt, McpServerCreateResponse, Resource};
use orch_authz::{
    Caller, Policy,
    ownership::{self, NewAccess},
};
use orch_types::Time;

const MCP: KeyTargetKind = KeyTargetKind::Object(ObjectKind::McpServer);
const INVALID_HOME: &str = "a preset is at home in an org";
const SYSTEM_ONLY: &str = "only a system admin makes a system MCP preset";
const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: Some(sources::GROUPS),
        nodes: [
            (ObjectKind::Org, sources::NODES[0]),
            (ObjectKind::Project, sources::NODES[1]),
            (ObjectKind::Topic, sources::NODES[2]),
            (ObjectKind::Thread, sources::NODES[3]),
        ],
        containers: None,
    },
    sources: sources::REFERENCES,
};
pub(super) struct Presets;
pub(super) struct Proposal {
    row: McpRow,
    access: NewAccess,
    secrets: BTreeMap<String, String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Created {
    id: Id,
    home: Option<Id>,
}
pub(super) struct Subject {
    created: Created,
    row: McpRow,
}

async fn require_home(conn: &mut Conn, principal: &Principal, home: Option<Id>) -> ApiResult<Home> {
    creation_home::require(
        conn,
        principal,
        ResourceKind::McpServer,
        home,
        INVALID_HOME,
        SYSTEM_ONLY,
    )
    .await
}

impl effects::Effect for Presets {
    type Source = Source;
    type Query = ();
    type Body = api::McpServerCreateBody;
    type Value = Proposal;
    type Output = Created;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let cleaned_name = name(&body.name)?;
            let cleaned_spec = spec(&body.spec)?;
            secret_values(&cleaned_spec, body.secrets.as_ref())?;
            let known = native::bind_known(
                ctx,
                &sources::CREATE,
                &[native::Facts::one(
                    Source::Home,
                    MCP,
                    creation_home::target(conn, body.home_id).await?,
                )],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_home(conn, &principal, body.home_id).await?;
            creation_home::lock(conn, body.home_id).await?;
            let home = require_home(conn, &principal, body.home_id).await?;
            rows::lock_preset_name(conn, home.org, None, &cleaned_name).await?;
            let row = McpRow {
                imported: false,
                id: orch_types::new_id(),
                name: cleaned_name,
                description: body.description.unwrap_or_default().trim().to_owned(),
                org_id: home.org,
                home_id: body.home_id,
                owner_id: Some(principal.user()),
                machine_id: None,
                environment_id: None,
                spec: cleaned_spec,
                enabled: true,
                revision: 1,
            };
            Ok((
                Proposal {
                    row,
                    access: match body.access {
                        None | Some(NewResourceAccess::Defaults) => NewAccess::Defaults,
                        Some(NewResourceAccess::OnlyMe) => NewAccess::OnlyMe,
                        Some(NewResourceAccess::Home) => NewAccess::Home,
                    },
                    secrets: body.secrets.unwrap_or_default(),
                },
                vec![native::Facts::one(
                    Source::Home,
                    MCP,
                    creation_home::target(conn, body.home_id).await?,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_home(conn, principal, value.row.home_id).await?;
            Ok(())
        })
    }
}

/// The original preset writer; no definition lookup or foreign work occurs after the check.
async fn insert<F>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Proposal,
    before: F,
) -> ApiResult<Created>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    before()?;
    rows::insert(conn, &value.row, ctx.now).await?;
    for (name, secret) in &value.secrets {
        put_secret(ctx, conn, value.row.id, name, secret).await?;
    }
    ownership::start_resource(
        conn,
        ObjectRef {
            kind: ObjectKind::McpServer,
            id: value.row.id,
        },
        principal.user(),
        value.access,
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::McpCreated),
        value.row.id,
        home(&value.row),
        "Added an MCP preset",
        json!({"server":value.row.id,"name":value.row.name}),
    )
    .await?;
    Ok(Created {
        id: value.row.id,
        home: value.row.home_id,
    })
}
impl effects::Apply for Presets {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Created>>> {
        Box::pin(async move {
            Ok(effects::Outcome::Applied(
                insert(ctx, conn, principal, value, move || before.check()).await?,
            ))
        })
    }
}

impl native::Stored for Presets {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::CREATE];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let created = *ctx.retained_source::<Created>()?;
            let row = rows::servers(conn, &[created.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let home = match created.home {
                Some(id) => crate::key_operations::targets::node_identity(conn, id).await?,
                None => crate::key_operations::targets::system(),
            };
            let object = ObjectRef {
                kind: ObjectKind::McpServer,
                id: created.id,
            };
            let target = single::subject(conn, object).await?;
            Ok((
                Subject { created, row },
                vec![
                    native::Facts::one(Source::Home, MCP, home),
                    native::Facts::one(Source::Created, MCP, target),
                ],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_home(conn, principal, value.created.home).await?;
            Ok(())
        })
    }
}
impl native::resource_reply::StoredResource for Presets {
    const READ_SOURCE: Source = Source::Created;
    const ROLES: &'static single::Roles = &ROLES;
    fn object(value: &Subject) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::McpServer,
            id: value.created.id,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match require_subject(
                conn,
                principal,
                value.created.id,
                value.row.environment_id,
                false,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(error)
                    if native::modern(principal.caller())
                        || !matches!(error.problem.status, 403 | 404) =>
                {
                    return Err(error);
                }
                Err(_) => {}
            }
            // Legacy creation keeps its generated native result through its original home gate.
            require_home(conn, principal, value.created.home).await?;
            let held = orch_authz::resources::effective(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::McpServer,
                    id: value.created.id,
                },
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            if !held.owner
                || value.row.home_id != value.created.home
                || value.row.imported
                || value.row.environment_id.is_some()
                || value.row.machine_id.is_some()
            {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.created == fresh.created
                && opened.row.home_id == fresh.row.home_id
                && opened.row.environment_id == fresh.row.environment_id
                && opened.row.machine_id == fresh.row.machine_id
                && opened.row.imported == fresh.row.imported)
        })
    }
}
fn receipt(id: Id) -> McpServerCreateResponse {
    McpServerCreateResponse::Receipt(McpServerCreateReceipt {
        projection: ReceiptProjection::Receipt,
        server_id: id,
    })
}
pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _body: api::McpServerCreateBody,
) -> ApiResult<McpServerCreateResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Presets>(&ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&completed.ctx, &receipt(completed.output.id))?;
    completed.ctx.retain_source(completed.output)?;
    Ok(
        match native::resource_reply::stored::<Presets>(&completed.ctx, &completed.accepted).await?
        {
            Some(Resource::McpServer(server)) => McpServerCreateResponse::Full(Box::new(server)),
            None => receipt(completed.output.id),
            Some(_) => {
                return Err(ApiError::internal(
                    "the native created preset response has another kind",
                ));
            }
        },
    )
}
