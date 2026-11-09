//! Native owned-server creation and shared readback of written and imported servers.

use super::*;

use crate::key_operations::native::effects::Apply;

use crate::key_operations::native::effects::Effect;

use crate::key_operations::native::effects::FirstEffect;

use crate::key_operations::native::effects::Outcome;

use crate::key_operations::native::resource_reply::ResourceProjection;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment_sources::MCP_CREATE;

use orch_api_contract::environment_sources::MCP_IMPORT;

use orch_api_contract::environment_sources::McpCreateSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

pub use orch_api_base::routes::environments::mcp::create_sources::Creation;

pub use orch_api_base::routes::environments::mcp::create_sources::insert_native;
pub(super) struct OwnServers;

pub(super) struct SavedServers;

pub(crate) struct Created {
    pub server: Id,
    pub environment: Id,
    owner: Id,
    org: Id,
    owning_environment: Option<Id>,
    machine: Option<Id>,
    imported: bool,
    declaration: &'static Declaration<Source>,
}

impl Created {
    pub(super) fn imported(environment: Id, row: &McpRow) -> ApiResult<Self> {
        Ok(Self {
            server: row.id,
            environment,
            owner: row
                .owner_id
                .ok_or_else(|| ApiError::internal("the imported server has no owner"))?,
            org: row
                .org_id
                .ok_or_else(|| ApiError::internal("the imported server has no org"))?,
            owning_environment: row.environment_id,
            machine: row.machine_id,
            imported: row.imported,
            declaration: &MCP_IMPORT,
        })
    }
    fn replay(environment: Id, row: &McpRow, imported: bool) -> ApiResult<Self> {
        if row.imported != imported || !imported && row.environment_id != Some(environment) {
            return Err(ApiError::not_found());
        }
        let mut actual = Self::imported(environment, row)?;
        actual.declaration = if imported { &MCP_IMPORT } else { &MCP_CREATE };
        Ok(actual)
    }
}

pub(super) const OWN_CREATION: &str = "environment_mcp_server";

pub(super) const IMPORT_CREATION: &str = "environment_mcp_import";

fn creation_kind<const IMPORTED: bool>() -> &'static str {
    if IMPORTED {
        IMPORT_CREATION
    } else {
        OWN_CREATION
    }
}

fn identity<const IMPORTED: bool>(
    value: &api::EnvironmentMcpCreateResponse,
) -> ApiResult<orch_db::idempotency::creation::Created> {
    let id = match value {
        api::EnvironmentMcpCreateResponse::Full(value) => value.id,
        api::EnvironmentMcpCreateResponse::Receipt(value) => value.server_id,
    };
    Ok(orch_db::idempotency::creation::Created {
        kind: creation_kind::<IMPORTED>().into(),
        id,
        data: Json::Null,
    })
}

pub(super) fn own_binding() -> crate::middleware::idempotency::Creation<
    (),
    api::EnvironmentMcpServerCreateBody,
    api::EnvironmentMcpCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: OWN_CREATION,
        identity: identity::<false>,
        replay: |ctx, (), _, created| replay::<false>(ctx, created),
    }
}

pub(super) fn import_binding() -> crate::middleware::idempotency::Registered<
    (),
    api::EnvironmentMcpImportCreateBody,
    api::EnvironmentMcpCreateResponse,
> {
    crate::middleware::idempotency::Registered::from(crate::middleware::idempotency::Creation {
        kind: IMPORT_CREATION,
        identity: identity::<true>,
        replay: |ctx, (), _, created| replay::<true>(ctx, created),
    })
    .command(crate::middleware::idempotency::commands::CreationCommand {
        kind: orch_link_contract::commands::CommandKind::McpImport,
        identity: |row| {
            let command: orch_link_contract::commands::McpImport =
                serde_json::from_value(row.body.clone()).map_err(ApiError::internal)?;
            Ok(Some(orch_db::idempotency::creation::Created {
                kind: IMPORT_CREATION.into(),
                id: command.server_id,
                data: json!({"environment_id":command.environment_id}),
            }))
        },
    })
}

fn replay<const IMPORTED: bool>(
    ctx: Ctx,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<api::EnvironmentMcpCreateResponse>> {
    Box::pin(async move {
        let environment: Id = serde_json::from_value(created.data["environment_id"].clone())
            .map_err(ApiError::internal)?;
        if created.kind != creation_kind::<IMPORTED>() || ctx.id("id")? != environment {
            return Err(ApiError::internal(
                "the accepted MCP creation association changed",
            ));
        }
        let receipt = api::EnvironmentMcpCreateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            server_id: created.id,
            environment_id: environment,
        };
        crate::key_operations::publication::receipt(&ctx, &receipt)?;
        let account = ctx.authenticated_user()?.id();
        if created.data.get("native_command").is_some() {
            let mut conn = ctx.app.db.acquire().await?;
            let _actual_command =
                crate::middleware::idempotency::commands::load(&ctx, &mut conn, &created).await?;
        }
        let full = crate::key_operations::completed_projection::run(&ctx, async {
            let mut conn = ctx.app.db.acquire().await?;
            let row = rows::servers(&mut conn, &[created.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let actual = Created::replay(environment, &row, IMPORTED)?;
            drop(conn);
            native::resource_reply::with_input_completed_replay::<SavedServers>(
                &ctx, account, actual,
            )
            .await
        })
        .await;
        match full {
            Ok(Some(orch_api_contract::resources::Resource::McpServer(value))) => {
                Ok(api::EnvironmentMcpCreateResponse::Full(Box::new(value)))
            }
            Ok(None) => Ok(api::EnvironmentMcpCreateResponse::Receipt(receipt)),
            Ok(Some(_)) => Err(ApiError::internal(
                "the created native MCP projection changed resource kind",
            )),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(api::EnvironmentMcpCreateResponse::Receipt(receipt))
            }
            Err(error) => Err(error),
        }
    })
}

pub(super) async fn parent_facts(
    conn: &mut Conn,
    environment: Id,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await?;
    Ok(vec![
        native::Facts::one(
            Source::Environment,
            KeyTargetKind::Object(ObjectKind::Environment),
            target.clone(),
        ),
        native::Facts::one(
            Source::Parent,
            KeyTargetKind::Object(ObjectKind::McpServer),
            target,
        ),
    ])
}

async fn require_create(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
) -> ApiResult<environments::EnvironmentState> {
    let gate = require(
        conn,
        principal,
        ResourceKind::Environment,
        environment,
        Permission::EnvironmentManage,
    )
    .await;
    if native::modern(principal.caller()) {
        gate.map_err(authorization::problem_without_content)?;
    } else {
        gate?;
    }
    let (env, _, policy) = machine_policy(conn, environment).await?;
    if !policy.mcp_add {
        return Err(ApiError::new(
            ErrorCode::PolicyMcpAddDisabled,
            "the machine accepts no tool server written from here",
        ));
    }
    Ok(env)
}
orch_api_base::__api_bind_a0c768943c!();

impl Effect for OwnServers {
    type Source = Source;
    type Query = ();
    type Body = api::EnvironmentMcpServerCreateBody;
    type Value = Creation;
    type Output = Created;
    fn declaration(_value: &Creation) -> &'static Declaration<Source> {
        &MCP_CREATE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Creation, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let facts = parent_facts(conn, environment).await?;
            let plan = native::bind_required(ctx, &MCP_CREATE, &facts)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            let value = prepare_native(conn, &principal, environment, body).await?;
            Ok((value, parent_facts(conn, environment).await?))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Creation,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let env = require_create(conn, principal, value.environment).await?;
            if env.org_id != value.org {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
}

impl Apply for OwnServers {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Creation,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Created>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let row = insert_native(ctx, conn, principal, value, move || before.check()).await?;
            let created = Created {
                server: row.id,
                environment: value.environment,
                owner: principal.user(),
                org: value.org,
                owning_environment: row.environment_id,
                machine: row.machine_id,
                imported: row.imported,
                declaration: &MCP_CREATE,
            };
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(principal.user()),
                OWN_CREATION,
                created.server,
                json!({"environment_id":value.environment}),
            )
            .await?;
            deadline.check()?;
            Ok(Outcome::Applied(created))
        })
    }
}

/// Current native environment and local-policy preparation, with no operation/path alias.
pub(crate) async fn prepare_native(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    body: api::EnvironmentMcpServerCreateBody,
) -> ApiResult<Creation> {
    let name = resources::name(&body.name)?;
    let spec = resources::spec(&body.spec)?;
    let typed: orch_agent_contract::discovery::McpServerSpec =
        serde_json::from_value(spec.clone()).map_err(ApiError::internal)?;
    let secrets = body.secrets.unwrap_or_default();
    if secrets.keys().any(|name| !typed.secret_env.contains(name)) {
        return Err(invalid(
            "secrets",
            "only names declared in secret_env are supplied here",
        ));
    }
    require_create(conn, principal, environment).await?;
    environments::lock_environment(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let env = require_create(conn, principal, environment).await?;
    Ok(Creation {
        environment,
        org: env.org_id,
        name,
        description: body.description.unwrap_or_default().trim().to_owned(),
        spec,
        secrets,
        enable: body.enable.unwrap_or(true),
    })
}
orch_api_base::__api_bind_be38dce02e!();

impl ResourceProjection for SavedServers {
    type Source = Source;
    type Value = McpRow;
    type Input = Created;
    const READ_SOURCE: Source = Source::Created;
    const ROLES: &'static single::Roles = &single::Roles {
        names: NameRoles {
            groups: Some("mcp_created_groups"),
            nodes: [
                (ObjectKind::Org, "mcp_created_orgs"),
                (ObjectKind::Project, "mcp_created_projects"),
                (ObjectKind::Topic, "mcp_created_topics"),
                (ObjectKind::Thread, "mcp_created_threads"),
            ],
            containers: None,
        },
        sources: "mcp_created_sources",
    };
    fn declaration(ctx: &Ctx, input: &Created) -> ApiResult<&'static Declaration<Source>> {
        if input.declaration.operation != ctx.route.name {
            return Err(ApiError::internal(
                "the created server changed its operation",
            ));
        }
        Ok(input.declaration)
    }
    fn load<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Created,
    ) -> BoxFuture<'a, ApiResult<(McpRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::servers(conn, &[input.server])
                .await?
                .into_iter()
                .next()
                .ok_or_else(ApiError::not_found)?;
            if row.environment_id != input.owning_environment
                || row.machine_id != input.machine
                || row.imported != input.imported
                || row.owner_id != Some(input.owner)
                || row.org_id != Some(input.org)
            {
                return Err(ApiError::not_found());
            }
            let mut facts = parent_facts(conn, input.environment).await?;
            let target = targets::page_object(
                conn,
                crate::routes::resources::mcp::projection::subject(&row),
            )
            .await?;
            facts.push(native::Facts::one(
                Source::Created,
                KeyTargetKind::Object(ObjectKind::McpServer),
                target,
            ));
            Ok((row, facts))
        })
    }
    fn object(value: &McpRow) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::McpServer,
            id: value.id,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a McpRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            resources::require_subject(conn, principal, value.id, value.environment_id, true).await
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a McpRow,
        fresh: &'a McpRow,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.id == fresh.id
                && opened.environment_id == fresh.environment_id
                && opened.owner_id == fresh.owner_id
                && opened.org_id == fresh.org_id
                && opened.home_id == fresh.home_id
                && opened.machine_id == fresh.machine_id
                && opened.imported == fresh.imported)
        })
    }
}
