//! Stored MCP changes consume their actual locked subject before one native write.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::middleware::transaction_guard;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::mcp_sources as sources;

use orch_api_contract::mcp_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::McpServerUpdateReceipt;

use orch_api_contract::resources::McpServerUpdateResponse;

use orch_api_contract::resources::Resource;

use orch_authz::Caller;
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

pub(super) struct Servers;

pub(super) struct Change {
    row: McpRow,
    enabled_before: bool,
    definition: bool,
    secrets: Vec<(String, Vec<u8>)>,
    stale: bool,
}

pub(super) struct Completion {
    id: Id,
    revision: i64,
    applied: bool,
}

async fn facts(
    conn: &mut Conn,
    id: Id,
    environment: Option<Id>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = single::subject(
        conn,
        ObjectRef {
            kind: ObjectKind::McpServer,
            id,
        },
    )
    .await?;
    let parent = match environment {
        Some(id) => vec![(
            KeyTargetKind::Object(ObjectKind::Environment),
            vec![
                crate::key_operations::targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id,
                    },
                )
                .await?,
            ],
        )],
        None => Vec::new(),
    };
    Ok(vec![
        native::Facts::one(
            Source::Subject,
            KeyTargetKind::Object(ObjectKind::McpServer),
            target,
        ),
        native::Facts::collection(Source::Parent, parent),
    ])
}

impl effects::Effect for Servers {
    type Source = Source;
    type Query = ();
    type Body = api::McpServerUpdateBody;
    type Value = Change;
    type Output = Completion;

    fn declaration(_value: &Change) -> &'static Declaration<Source> {
        &sources::UPDATE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Change, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let clean_name = body.name.as_deref().map(name).transpose()?;
            let clean_spec = body.spec.as_ref().map(spec).transpose()?;
            let id = ctx.id("id")?;
            let selected = rows::subject(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let known = native::bind_known(
                ctx,
                &sources::UPDATE,
                &facts(conn, id, selected.environment_id).await?,
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_subject(conn, &principal, id, selected.environment_id, true).await?;
            let mut row = rows::lock_update(conn, id, clean_name.as_deref())
                .await?
                .ok_or_else(ApiError::not_found)?;
            let current = facts(conn, id, row.environment_id).await?;
            let known = native::bind_known(ctx, &sources::UPDATE, &current)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_row(conn, &principal, &row, true).await?;
            let stale = row.revision != body.expected_revision;
            let enabled_before = row.enabled;
            let definition = body.spec.is_some();
            let mut secrets = Vec::new();
            if !stale {
                if row.imported && (body.spec.is_some() || body.secrets.is_some()) {
                    return Err(invalid(
                        "spec",
                        "an imported definition changes only by reimporting it from its machine",
                    ));
                }
                if let Some(name) = clean_name {
                    row.name = name;
                }
                if let Some(description) = body.description {
                    row.description = description.trim().to_owned();
                }
                if let Some(enabled) = body.enabled {
                    row.enabled = enabled;
                }
                if let Some(spec) = clean_spec {
                    row.spec = spec;
                }
                secret_values(&row.spec, body.secrets.as_ref())?;
                for (name, value) in body.secrets.into_iter().flatten() {
                    secrets.push((name.clone(), seal_secret(row.id, &name, &value)?));
                }
                let names: Vec<_> = secrets.iter().map(|(name, _)| name.as_str()).collect();
                sqlx::query("SELECT id FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name=ANY($2) ORDER BY name FOR UPDATE")
                    .bind(id).bind(names).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            }
            Ok((
                Change {
                    row,
                    enabled_before,
                    definition,
                    secrets,
                    stale,
                },
                current,
            ))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Change,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_row(conn, principal, &value.row, true))
    }
}

impl effects::Apply for Servers {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Change,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Completion>>> {
        Box::pin(async move {
            if value.stale {
                return Ok(effects::Outcome::Unchanged(Completion {
                    id: value.row.id,
                    revision: value.row.revision,
                    applied: false,
                }));
            }
            let row = &value.row;
            let revision = row
                .revision
                .checked_add(1)
                .ok_or_else(|| ApiError::internal("the native server revision cannot advance"))?;
            before.check()?;
            for (name, sealed) in &value.secrets {
                orch_db::queries::settings::put_secret(
                    conn,
                    "mcp_server",
                    row.id,
                    name,
                    sealed,
                    Some(principal.user()),
                    ctx.now,
                )
                .await?;
            }
            rows::update(conn, row, ctx.now).await?;
            if !value.enabled_before && row.enabled {
                record(
                    ctx,
                    conn,
                    orch_db::audit::action!(AuditAction::McpEnabled),
                    row.id,
                    home(row),
                    "Enabled an MCP server",
                    json!({"server":row.id,"name":row.name}),
                )
                .await?;
            }
            record(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::McpUpdated),
                row.id,
                home(row),
                "Changed an MCP server",
                json!({"server":row.id,"name":row.name,"definition":value.definition,"enabled":row.enabled}),
            )
            .await?;
            Ok(effects::Outcome::Applied(Completion {
                id: row.id,
                revision,
                applied: true,
            }))
        })
    }
}

impl native::Stored for Servers {
    type Source = Source;
    type Value = McpRow;
    const ROWS: &'static [Declaration<Source>] = &[sources::UPDATE];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(McpRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::servers(conn, &[ctx.id("id")?])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, row.id, row.environment_id).await?;
            Ok((row, facts))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a McpRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_row(conn, principal, row, true))
    }
}

impl native::resource_reply::StoredResource for Servers {
    const READ_SOURCE: Source = Source::Subject;
    const ROLES: &'static single::Roles = &ROLES;

    fn object(row: &McpRow) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::McpServer,
            id: row.id,
        }
    }

    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a McpRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match require_row(conn, principal, row, false).await {
                Ok(()) => Ok(()),
                Err(error)
                    if native::modern(principal.caller())
                        || !matches!(error.problem.status, 403 | 404) =>
                {
                    Err(error)
                }
                Err(_) => require_row(conn, principal, row, true).await,
            }
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
                && opened.home_id == fresh.home_id
                && opened.org_id == fresh.org_id
                && opened.environment_id == fresh.environment_id
                && opened.machine_id == fresh.machine_id
                && opened.imported == fresh.imported)
        })
    }
}

fn receipt(completed: &Completion) -> McpServerUpdateResponse {
    McpServerUpdateResponse::Receipt(McpServerUpdateReceipt {
        projection: ReceiptProjection::Receipt,
        server_id: completed.id,
        revision: completed.revision,
    })
}

pub(super) async fn update(
    ctx: Ctx,
    (): (),
    _body: api::McpServerUpdateBody,
) -> ApiResult<McpServerUpdateResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Servers>(&ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    if completed.output.applied {
        crate::key_operations::publication::receipt(&completed.ctx, &receipt(&completed.output))?;
    }
    let current =
        match native::resource_reply::stored::<Servers>(&completed.ctx, &completed.accepted).await?
        {
            Some(Resource::McpServer(server)) => Some(server),
            None => None,
            Some(_) => {
                return Err(ApiError::internal(
                    "the native changed MCP response has another kind",
                ));
            }
        };
    if !completed.output.applied {
        let error = ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the server changed since it was read",
        );
        return Err(match current {
            Some(server) => error.with("current", server),
            None => error,
        });
    }
    Ok(match current {
        Some(server) => McpServerUpdateResponse::Full(Box::new(server)),
        None => receipt(&completed.output),
    })
}
