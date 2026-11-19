//! Native owned-server creation and the shared immutable-created-input resource readback.

use super::*;
use crate::key_operations::native::effects::{Apply, Effect, FirstEffect, Outcome};
use crate::key_operations::native::resource_reply::ResourceProjection;
use crate::key_operations::{native, targets};
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::projection::single;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::environment_sources::{MCP_CREATE, McpCreateSource as Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;
use std::collections::BTreeMap;

pub(super) struct OwnServers;

pub(super) struct Creation {
    environment: Id,
    org: Id,
    name: String,
    description: String,
    spec: Json,
    secrets: BTreeMap<String, String>,
    enable: bool,
}

pub(super) struct Created {
    pub server: Id,
    pub environment: Id,
    owner: Id,
    org: Id,
}

async fn parent_facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
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
            let facts = parent_facts(conn, environment).await?;
            let plan = native::bind_required(ctx, &MCP_CREATE, &facts)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            require_create(conn, &principal, environment).await?;
            environments::lock_environment(conn, environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let env = require_create(conn, &principal, environment).await?;
            Ok((
                Creation {
                    environment,
                    org: env.org_id,
                    name,
                    description: body.description.unwrap_or_default().trim().to_owned(),
                    spec,
                    secrets,
                    enable: body.enable.unwrap_or(true),
                },
                parent_facts(conn, environment).await?,
            ))
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
            before.check()?;
            let row = McpRow {
                imported: false,
                id: orch_types::new_id(),
                name: value.name.clone(),
                description: value.description.clone(),
                org_id: Some(value.org),
                home_id: Some(value.org),
                owner_id: Some(principal.user()),
                machine_id: None,
                environment_id: Some(value.environment),
                spec: value.spec.clone(),
                enabled: true,
                revision: 1,
            };
            rows::insert(conn, &row, ctx.now).await?;
            for (name, value) in &value.secrets {
                resources::put_secret(ctx, conn, row.id, name, value).await?;
            }
            rows::enable(conn, value.environment, row.id, value.enable, ctx.now).await?;
            record(
                ctx,
                conn,
                AuditAction::McpCreated,
                row.id,
                Home {
                    node: row.home_id,
                    org: row.org_id,
                },
                "Added an environment MCP server",
                json!({"server":row.id,"environment":value.environment,"name":row.name}),
            )
            .await?;
            Ok(Outcome::Applied(Created {
                server: row.id,
                environment: value.environment,
                owner: principal.user(),
                org: value.org,
            }))
        })
    }
}

impl ResourceProjection for OwnServers {
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
    fn declaration(_ctx: &Ctx, _input: &Created) -> ApiResult<&'static Declaration<Source>> {
        Ok(&MCP_CREATE)
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
            if row.environment_id != Some(input.environment)
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
                && opened.home_id == fresh.home_id)
        })
    }
}
