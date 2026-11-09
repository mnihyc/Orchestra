//! Native MCP copies carry only an actual owner's declared secret values through the native sealing writer.
use super::*;
use crate::{
    key_operations::native::{self, effects},
    routes::{
        environments::mcp::create_sources as owned,
        resources::mcp::{self as mcp, creation},
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, ResourceKind},
};
use orch_api_contract::{
    duplicate_sources::{self as sources, Source},
    duplicates::{
        CopySubject, CopySubjectKind, McpDuplicateBody, McpMoveOutBody, McpServerMoveOut,
        McpServerMoveOutPreview, MoveOutDeleteEffect, MoveOutDeletePlan,
    },
    operation_sources::Declaration,
    resources::McpServerCreateBody,
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::resources::{grouping, mcp as rows},
};
mod imported;
pub(super) struct Presets;
pub(super) struct Moving;
struct Preview;
impl CreationIdentity for Moving {
    const CREATED_KIND: DuplicateKind = DuplicateKind::McpServer;
    const CREATED_REPLAY: &'static str = "mcp_move_out";
}
impl CreationIdentity for Presets {
    const CREATED_KIND: DuplicateKind = DuplicateKind::McpServer;
    const CREATED_REPLAY: &'static str = "mcp_duplicate";
}
pub(super) fn mount(registry: &mut crate::registry::Registry) {
    registry.on_with_operation(
        orch_api_contract::duplicates::McpServerDuplicate,
        effects::prepare::<Presets>,
        imported::run,
    );
    registry.creation(
        orch_api_contract::duplicates::McpServerDuplicate,
        imported::binding::<Presets, McpDuplicateBody>(),
    );
    registry
        .on_with_operation(
            McpServerMoveOut,
            effects::prepare::<Moving>,
            imported::run_move,
        )
        .on_snapshot_body_read_with_optional_operation(
            McpServerMoveOutPreview,
            preview_prepare,
            preview,
        )
        .creation(
            McpServerMoveOut,
            imported::binding::<Moving, McpMoveOutBody>(),
        );
}
enum New {
    Preset(Box<creation::Proposal>),
    Owned(Box<owned::Creation>),
    Imported(Box<orch_engine::environment_mcp::duplicating::Input>),
}
pub(super) struct Prepared {
    home: Option<Id>,
    new: New,
    row: rows::McpRow,
    owner: ObjectRef,
    secrets_copied: bool,
    tags: Vec<Id>,
    delete: Option<MoveOutDeletePlan>,
    ending: Option<orch_engine::environment_mcp::deleting::Deletion>,
}
enum Intent {
    Copy(McpDuplicateBody),
    Preview(McpDuplicateBody),
    Move(McpMoveOutBody),
}
impl Intent {
    fn declaration(&self) -> &'static Declaration<Source> {
        match self {
            Self::Copy(_) => &sources::MCP,
            Self::Preview(_) => &sources::MCP_MOVE_PREVIEW,
            Self::Move(_) => &sources::MCP_MOVE,
        }
    }
}
async fn prepare_native(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    intent: Intent,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let preview = matches!(&intent, Intent::Preview(_));
    let writing = orch_db::queries::transaction_guard::in_transaction(conn);
    let moving = !matches!(&intent, Intent::Copy(_));
    let declaration = intent.declaration();
    let (body, confirmation) = match intent {
        Intent::Copy(body) | Intent::Preview(body) => (body, None),
        Intent::Move(body) => (body.copy, Some(body.delete_plan)),
    };
    let target_machine = body.machine_id;
    let body: DuplicateBody = body.into();
    let source = ctx.id("id")?;
    if moving && !preview && writing {
        rows::lock_definition(conn, source).await?;
    }
    let row = if preview {
        rows::servers(conn, &[source]).await?.pop()
    } else {
        rows::lock_update(conn, source, None).await?
    }
    .ok_or_else(ApiError::not_found)?;
    let home = destination(&body, row.home_id);
    let at = row.environment_id.filter(|_| body.destination.is_none());
    // Resolve the actual parent FIRST: an owned child has no flat resource page.
    let governor = mcp::projection::subject(&row);
    let destination = match at {
        Some(id) => {
            crate::key_operations::targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?
        }
        None => match home {
            Some(id) => crate::key_operations::targets::node(conn, id).await?,
            None => crate::key_operations::targets::system(),
        },
    };
    let mut facts = vec![
        native::Facts::one(
            Source::Subject,
            orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::McpServer),
            crate::key_operations::targets::page_object(conn, governor).await?,
        ),
        native::Facts::one(
            Source::Destination,
            orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::McpServer),
            destination,
        ),
    ];
    facts.push(environment_facts(conn, Source::DestinationEnvironments, at).await?);
    if moving {
        facts.push(environment_facts(conn, Source::SourceEnvironments, row.environment_id).await?);
    }
    if !row.imported {
        facts.push(native::Facts::collection(Source::Machine, Vec::new()));
    }
    let plan = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    mcp::require_row(conn, &principal, &row, moving).await?;
    revision(&body, row.revision)?;
    if at.is_none() {
        creation::require_home(conn, &principal, home).await?;
    }
    let org = copy_to(
        conn,
        &principal,
        subject(DuplicateKind::McpServer, row.id),
        home,
    )
    .await?;
    let delete = if moving {
        if !owns(conn, &principal, mcp::projection::subject(&row)).await? {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "Only a current owner can move this MCP definition to another org.",
            ));
        }
        let (_, source_org) =
            orch_engine::copy_out::source_place(conn, subject(DuplicateKind::McpServer, row.id))
                .await
                .map_err(copy_error)?;
        if source_org == org {
            return Err(ApiError::invalid(
                "destination",
                "Use the existing Move within this organization.",
            ));
        }
        let copies = orch_engine::environment_mcp::deleting::copies(conn, row.id)
            .await
            .map_err(ApiError::engine)?;
        facts.push(deleted_copy_facts(conn, &copies).await?);
        if preview {
            facts.push(native::Facts::one(
                Source::PreviewContent,
                orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::McpServer),
                crate::key_operations::targets::page_object(conn, mcp::projection::subject(&row))
                    .await?,
            ));
        }
        let plan = deletion_plan(&row, &principal, &body, home, target_machine, &copies)?;
        if let Some(confirmation) = &confirmation {
            super::moving::confirm(&plan, confirmation)?;
        }
        Some(plan)
    } else {
        None
    };
    let mut ending = None;
    let name = match body.name {
        Some(name) => name,
        None => copy_name(ctx, conn, principal.caller(), org, at, &row.name).await?,
    };
    let object = match row.environment_id {
        Some(id) => ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
        None => ObjectRef {
            kind: ObjectKind::McpServer,
            id: row.id,
        },
    };
    if row.imported {
        use orch_engine::environment_mcp::duplicating::{Input, Stored};
        use orch_engine::operation_scope::continuing::Source as _;
        let candidate = target_machine
            .or(row.machine_id)
            .ok_or_else(ApiError::not_found)?;
        facts.push(native::Facts::collection(
            Source::Machine,
            vec![(
                orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Machine),
                vec![crate::key_operations::targets::machine_subject(conn, candidate).await?],
            )],
        ));
        let full = native::bind_required(ctx, declaration, &facts)?;
        let principal = native::preparation_principal(ctx, conn, caller, &full).await?;
        if !orch_engine::environment_mcp::duplicating::machine_allowed(
            conn, &principal, candidate, home,
        )
        .await
        .map_err(imported::source_error)?
        {
            return Err(crate::routes::nodes::invalid(
                "machine_id",
                "Choose a machine usable by the destination organization.",
            ));
        }
        if moving && !preview && writing {
            let copies = orch_engine::environment_mcp::deleting::copies(conn, row.id)
                .await
                .map_err(ApiError::engine)?;
            ending = Some(
                orch_engine::environment_mcp::deleting::Deletion::capture(
                    conn,
                    principal.caller(),
                    &row,
                    &copies,
                    orch_types::Time::new(ctx.now),
                    &crate::sign_in::origin(ctx),
                )
                .await
                .map_err(ApiError::engine)?,
            );
        }
        let include_secrets = owns(conn, &principal, object).await?;
        let input = match ctx.optional_retained_source::<Input>()? {
            Some(input)
                if input.preserves_request(row.id, home, &name, target_machine)
                    && input.moving() == (moving && !preview) =>
            {
                (*input).clone()
            }
            Some(_) => {
                return Err(ApiError::internal(
                    "the native MCP duplicate request changed",
                ));
            }
            None => {
                let input =
                    Input::capture(conn, &row, home, org, name, include_secrets, target_machine)
                        .await
                        .map_err(ApiError::engine)?;
                match ending.clone() {
                    Some(ending) => input.with_ending(ending).map_err(ApiError::engine)?,
                    None => input,
                }
            }
        };
        if !input
            .destination_allowed(conn, &principal)
            .await
            .map_err(imported::source_error)?
        {
            return Err(crate::routes::nodes::invalid(
                "machine_id",
                "Choose a machine usable by the destination organization.",
            ));
        }
        let (_, mut native_facts) = if input.moving() {
            orch_engine::environment_mcp::duplicating::Moving::prepare(
                conn,
                caller,
                &input,
                ctx.now.into(),
            )
            .await
        } else {
            Stored::prepare(conn, caller, &input, ctx.now.into()).await
        }
        .map_err(imported::source_error)?;
        if moving && !input.moving() {
            native_facts.push(
                environment_facts(conn, Source::SourceEnvironments, row.environment_id).await?,
            );
            let copies = orch_engine::environment_mcp::deleting::copies(conn, row.id)
                .await
                .map_err(ApiError::engine)?;
            native_facts.push(deleted_copy_facts(conn, &copies).await?);
            if preview {
                native_facts.push(native::Facts::one(
                    Source::PreviewContent,
                    orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::McpServer),
                    crate::key_operations::targets::page_object(
                        conn,
                        mcp::projection::subject(&row),
                    )
                    .await?,
                ));
            }
        }
        current(ctx, principal.caller())?;
        return Ok((
            Prepared {
                home,
                new: New::Imported(Box::new(input)),
                row,
                owner: object,
                secrets_copied: include_secrets,
                tags: Vec::new(),
                delete,
                ending,
            },
            native_facts,
        ));
    }
    if target_machine.is_some() {
        return Err(crate::routes::nodes::invalid(
            "machine_id",
            "only an imported snapshot needs a destination machine",
        ));
    }
    if moving && !preview && writing {
        let full = native::bind_required(ctx, declaration, &facts)?;
        let principal = native::preparation_principal(ctx, conn, caller, &full).await?;
        let copies = orch_engine::environment_mcp::deleting::copies(conn, row.id)
            .await
            .map_err(ApiError::engine)?;
        ending = Some(
            orch_engine::environment_mcp::deleting::Deletion::capture(
                conn,
                principal.caller(),
                &row,
                &copies,
                orch_types::Time::new(ctx.now),
                &crate::sign_in::origin(ctx),
            )
            .await
            .map_err(ApiError::engine)?,
        );
    }
    let values = if owns(conn, &principal, object).await? {
        let names = row
            .spec
            .get("secret_env")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        secret_values(conn, "mcp_server", row.id, &names).await?
    } else {
        Default::default()
    };
    let secrets_copied = !values.is_empty();
    let new = match at {
        None => New::Preset(Box::new(
            creation::prepare_native(
                conn,
                &principal,
                McpServerCreateBody {
                    name,
                    description: Some(row.description.clone()),
                    home_id: home,
                    spec: row.spec.clone(),
                    secrets: Some(values),
                    access: None,
                },
            )
            .await?,
        )),
        Some(environment) => New::Owned(Box::new(
            owned::prepare_native(
                conn,
                &principal,
                environment,
                orch_api_contract::environment::EnvironmentMcpServerCreateBody {
                    name,
                    description: Some(row.description.clone()),
                    spec: row.spec.clone(),
                    secrets: Some(values),
                    enable: Some(true),
                },
            )
            .await?,
        )),
    };
    let tags = copy_tags(conn, ResourceKind::McpServer, row.id, home).await?;
    current(ctx, principal.caller())?;
    Ok((
        Prepared {
            home,
            new,
            secrets_copied,
            tags,
            row,
            owner: object,
            delete,
            ending,
        },
        facts,
    ))
}
async fn require_native(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Prepared,
) -> ApiResult<()> {
    mcp::require_row(conn, principal, &value.row, value.delete.is_some()).await?;
    if value.delete.is_some() && !owns(conn, principal, value.owner).await? {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "Only a current owner can complete this native MCP move.",
        ));
    }
    if let Some(ending) = &value.ending
        && !ending
            .matches_source(conn)
            .await
            .map_err(ApiError::engine)?
    {
        return Err(super::moving::stale());
    }
    copy_to(
        conn,
        principal,
        subject(DuplicateKind::McpServer, value.row.id),
        value.home,
    )
    .await?;
    match &value.new {
        New::Preset(_) => {
            creation::require_home(conn, principal, value.home).await?;
        }
        New::Owned(proposal) => proposal.require(conn, principal).await?,
        New::Imported(input) => {
            use orch_engine::environment_mcp::duplicating::Stored;
            use orch_engine::operation_scope::continuing::Source as _;
            if input.moving() {
                type Source = orch_engine::environment_mcp::duplicating::Moving;
                let (row, _) = Source::prepare(conn, principal.caller(), input, _ctx.now.into())
                    .await
                    .map_err(imported::source_error)?;
                Source::locks(conn, principal, input, &row)
                    .await
                    .map_err(imported::source_error)?;
                let (row, _) = Source::prepare(conn, principal.caller(), input, _ctx.now.into())
                    .await
                    .map_err(imported::source_error)?;
                Source::require(conn, principal, input, &row)
                    .await
                    .map_err(imported::source_error)?;
            } else {
                let (row, _) = Stored::prepare(conn, principal.caller(), input, _ctx.now.into())
                    .await
                    .map_err(imported::source_error)?;
                Stored::locks(conn, principal, input, &row)
                    .await
                    .map_err(imported::source_error)?;
                let (row, _) = Stored::prepare(conn, principal.caller(), input, _ctx.now.into())
                    .await
                    .map_err(imported::source_error)?;
                Stored::require(conn, principal, input, &row)
                    .await
                    .map_err(imported::source_error)?;
            }
        }
    }
    if value.secrets_copied && !owns(conn, principal, value.owner).await? {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "only a current owner carries the source's credential values into a copy",
        ));
    }
    Ok(())
}
macro_rules! mcp_effect {
    ($family:ty,$body:ty,$declaration:expr,$intent:ident) => {
        impl effects::Effect for $family {
            type Source = Source;
            type Query = ();
            type Body = $body;
            type Value = Prepared;
            type Output = Id;
            fn declaration(_: &Prepared) -> &'static Declaration<Source> {
                $declaration
            }
            fn prepare<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                caller: &'a Caller,
                (): (),
                body: $body,
            ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
                Box::pin(prepare_native(ctx, conn, caller, Intent::$intent(body)))
            }
            fn require<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Prepared,
            ) -> BoxFuture<'a, ApiResult<()>> {
                Box::pin(require_native(ctx, conn, principal, value))
            }
        }
    };
}
mcp_effect!(Presets, McpDuplicateBody, &sources::MCP, Copy);
mcp_effect!(Moving, McpMoveOutBody, &sources::MCP_MOVE, Move);

impl Presets {
    fn apply_sql<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let id = match &value.new {
                New::Preset(proposal) => {
                    creation::insert(ctx, conn, principal, proposal, move || before.check())
                        .await?
                        .id()
                }
                New::Owned(proposal) => {
                    owned::insert_native(ctx, conn, principal, proposal, move || before.check())
                        .await?
                        .id
                }
                New::Imported(_) => {
                    return Err(ApiError::internal(
                        "a native stored copy requires its owned IO boundary",
                    ));
                }
            };
            deadline.check()?;
            if !value.tags.is_empty() {
                grouping::set_tags(conn, ResourceKind::McpServer, id, &value.tags, ctx.now).await?;
                deadline.check()?;
            }
            record_copy(ctx, conn, DuplicateKind::McpServer, value.row.id, id).await?;
            deadline.check()?;
            if let Some(ending) = &value.ending {
                ending
                    .apply(conn, principal, ctx.app.clock.as_ref())
                    .await
                    .map_err(ApiError::engine)?;
                deadline.check()?;
            }
            Ok(effects::Outcome::Applied(id))
        })
    }
}
async fn copy_name(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    org: Option<Id>,
    environment: Option<Id>,
    name: &str,
) -> ApiResult<String> {
    let mut number = 1_u64;
    loop {
        let suffix = if number == 1 {
            "-copy".to_owned()
        } else {
            format!("-copy-{number}")
        };
        let base: String = name.chars().take(32 - suffix.len()).collect();
        let candidate = format!("{base}{suffix}");
        rows::lock_preset_name(conn, org, environment, &candidate).await?;
        current(ctx, caller)?;
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE org_id IS NOT DISTINCT FROM $1 AND environment_id IS NOT DISTINCT FROM $2 AND NOT imported AND name=$3)").bind(org).bind(environment).bind(&candidate).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        current(ctx, caller)?;
        if !exists {
            return Ok(candidate);
        }
        number = number
            .checked_add(1)
            .ok_or_else(|| ApiError::internal("the copy name space is exhausted"))?;
    }
}

async fn environment_facts(
    conn: &mut Conn,
    source: Source,
    environment: Option<Id>,
) -> ApiResult<native::Facts<Source>> {
    let groups = match environment {
        None => Vec::new(),
        Some(id) => vec![(
            orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Environment),
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
    };
    Ok(native::Facts::collection(source, groups))
}

async fn deleted_copy_facts(
    conn: &mut Conn,
    copies: &[rows::Offering],
) -> ApiResult<native::Facts<Source>> {
    let ids: Vec<_> = copies.iter().map(|copy| copy.machine_id).collect();
    let mut actual = crate::key_operations::targets::machine_subjects(conn, &ids).await?;
    let selected = ids
        .iter()
        .map(|id| actual.remove(id).ok_or_else(ApiError::not_found))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(native::Facts::collection(
        Source::ForgottenCopies,
        if selected.is_empty() {
            vec![]
        } else {
            vec![(
                orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Machine),
                selected,
            )]
        },
    ))
}
fn deletion_plan(
    row: &rows::McpRow,
    principal: &Principal,
    body: &DuplicateBody,
    home: Option<Id>,
    machine: Option<Id>,
    copies: &[rows::Offering],
) -> ApiResult<MoveOutDeletePlan> {
    let mut footprint:Vec<_>=copies.iter().map(|copy|(copy.machine_id,serde_json::json!({"machine":copy.machine_id,"environment":copy.environment_id,"source":copy.source}))).collect();
    footprint.sort_by_key(|copy| copy.0);
    super::moving::plan(
        CopySubject {
            kind: CopySubjectKind::McpServer,
            id: row.id,
        },
        home.map_or(DuplicateDestination::System, |id| {
            DuplicateDestination::Node { id }
        }),
        row.revision,
        principal.user(),
        body,
        serde_json::json!({"home":row.home_id,"org":row.org_id,"owner":row.owner_id,"environment":row.environment_id,"machine":row.machine_id,"selected_machine":machine,"imported":row.imported,"spec":row.spec,"copies":footprint}),
        vec![MoveOutDeleteEffect::RemoveSource],
    )
}
fn preview_prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: McpDuplicateBody,
) -> crate::registry::OptionalOperationFuture<'a> {
    native::selected::<Preview>(ctx, conn, caller, body, ())
}
async fn preview(ctx: Ctx, (): (), body: McpDuplicateBody) -> ApiResult<MoveOutDeletePlan> {
    native::selected_read::<Preview>(ctx, body, ()).await
}
impl native::Selected for Preview {
    type Source = Source;
    type Query = McpDuplicateBody;
    type Value = Prepared;
    type Reply = MoveOutDeletePlan;
    const DECLARATION: &'static Declaration<Source> = &sources::MCP_MOVE_PREVIEW;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        body: &'a McpDuplicateBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_native(
            ctx,
            conn,
            caller,
            Intent::Preview(body.clone()),
        ))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<MoveOutDeletePlan>> {
        Box::pin(async move {
            require_native(ctx, conn, principal, value).await?;
            value
                .delete
                .clone()
                .ok_or_else(|| ApiError::internal("the MCP native move has no delete plan"))
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a McpDuplicateBody,
        old: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.row.id == fresh.row.id && old.home == fresh.home && old.delete == fresh.delete)
        })
    }
}
