//! A driver's import proposes a validated recipe and explains what it read, without saving a template.

use super::clean_driver;

use super::clean_spec;

use super::creation_authority;

use super::unit_word;

use super::version_key;

use crate::dispatch::StoredCommand;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::publication;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::authentication::Via;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::registry::Ctx;

use crate::routes::nodes::invalid;

use futures::future::BoxFuture;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::FieldFault;

use orch_api_contract::common::Problem;

use orch_api_contract::resources as api;

use orch_api_contract::resources::UnitKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::template_import_sources as declared;

use orch_api_contract::template_import_sources::Source as Role;

use orch_authz::usable;

use orch_authz::usable::Policies;

use orch_authz::usable::Usable;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::queries::resources::template_imports;

use orch_db::Conn;

use orch_db::DbError;

use orch_driver_contract::imports::ImportDef;

use orch_driver_contract::imports::archive_references;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::Outgoing;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::continuing::Source;

use orch_engine::template_imports as native_imports;

use orch_engine::template_imports::Input;

use orch_engine::template_imports::Proposal;

use orch_link_contract::commands::NodeTemplateImport;

use orch_link_contract::commands::NodeTemplateImportResult;

use orch_link_contract::commands::NoteLevel;

use orch_link_contract::commands::RuntimeKind;

use orch_link_contract::commands::SerialKey;

use orch_types::FieldType;

use orch_types::Json;

use orch_types::Time;
/// Proposes a recipe on an online host the caller may use in the requested home.
async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: api::TemplateImportBody,
) -> ApiResult<Input> {
    let driver = clean_driver("driver", &body.driver)?;
    let kind = body.kind.unwrap_or(UnitKind::Environment);
    let initial = native_imports::known_facts(conn, body.home_id, &driver, None)
        .await
        .map_err(ApiError::engine)?;
    let known = native::bind_known(ctx, &declared::IMPORT, &initial)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    creation_authority(conn, &principal, body.home_id, &driver).await?;
    let mut candidates = template_imports::candidates(conn).await?;
    candidates.sort_by_cached_key(|candidate| {
        std::cmp::Reverse(version_key(
            candidate
                .drivers
                .get(&driver)
                .and_then(|descriptor| descriptor.get("version"))
                .and_then(Json::as_str)
                .unwrap_or_default(),
        ))
    });
    let mut chosen = None;
    let mut scope_refusal = None;
    for candidate in candidates {
        let Some(descriptor) = candidate.drivers.get(&driver) else {
            continue;
        };
        if descriptor
            .get("spec_schemas")
            .and_then(|schemas| schemas.get(unit_word(kind)))
            .is_none()
        {
            continue;
        }
        let import = descriptor
            .get("imports")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .find(|import| {
                import.get("format").and_then(Json::as_str) == Some(body.format.as_str())
            });
        let Some(import) = import else {
            continue;
        };
        let facts = native_imports::known_facts(conn, body.home_id, &driver, Some(candidate.host))
            .await
            .map_err(ApiError::engine)?;
        let known = native::bind_known(ctx, &declared::IMPORT, &facts)?;
        let principal = match native::preparation_principal(ctx, conn, caller, &known).await {
            Ok(principal) => principal,
            Err(error) if error.problem.status == 403 => {
                scope_refusal = Some(error);
                continue;
            }
            Err(error) => return Err(error),
        };
        if !matches!(
            usable::usable(
                conn,
                &principal,
                candidate.host,
                body.home_id,
                &Policies::default()
            )
            .await
            .map_err(ApiError::internal)?,
            Usable::Yes(_)
        ) {
            continue;
        }
        let import: ImportDef = serde_json::from_value(import.clone())
            .map_err(|_| invalid("format", "the driver's import declaration is not usable"))?;
        chosen = Some((
            candidate.host,
            candidate.machine,
            import,
            descriptor.clone(),
        ));
        break;
    }
    let (host, machine, import, descriptor) = chosen.ok_or_else(|| {
        scope_refusal.unwrap_or_else(|| {
            ApiError::new(
                ErrorCode::SourceOffline,
                "no usable online host offers this driver's import for this pool",
            )
        })
    })?;
    let mut values = serde_json::Map::new();
    for (name, value) in &body.values {
        let field = import.fields.iter().find(|field| field.name == *name);
        let value = match field.map(|field| field.kind) {
            Some(FieldType::Json) => serde_json::from_str(value)
                .map_err(|_| invalid(&format!("values.{name}"), "must be valid JSON"))?,
            Some(FieldType::Boolean) => value
                .parse::<bool>()
                .map(Json::Bool)
                .map_err(|_| invalid(&format!("values.{name}"), "must be true or false"))?,
            Some(FieldType::Map | FieldType::SecretMap) => {
                serde_json::from_str(value).map_err(|_| {
                    invalid(&format!("values.{name}"), "must be names with text values")
                })?
            }
            _ => Json::String(value.clone()),
        };
        values.insert(name.clone(), value);
    }
    for field in &import.fields {
        if !values.contains_key(&field.name)
            && let Some(default) = &field.default
        {
            values.insert(field.name.clone(), default.clone());
        }
    }
    let values = Json::Object(values);
    import.check_values(&values).map_err(|faults| {
        ApiError::from(Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: format!("values.{}", fault.path),
                    message: fault.message,
                })
                .collect(),
        ))
    })?;
    let archives = archive_references(&values).map_err(|faults| {
        ApiError::from(Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: format!("values.{}", fault.path),
                    message: fault.message,
                })
                .collect(),
        ))
    })?;
    for archive in archives {
        if !orch_db::queries::uploads::owns(conn, principal.user(), &archive.hash).await? {
            return Err(invalid(
                &format!("values.{}", archive.name),
                "this caller has no completed upload of that archive",
            ));
        }
    }
    Ok(Input::new(
        body.home_id,
        host,
        machine,
        descriptor,
        NodeTemplateImport {
            driver,
            kind: match kind {
                UnitKind::Environment => RuntimeKind::Environment,
                UnitKind::Provider => RuntimeKind::Provider,
            },
            format: body.format,
            values,
        },
    ))
}

pub(super) struct Import;

impl effects::Effect for Import {
    type Source = Role;
    type Query = ();
    type Body = api::TemplateImportBody;
    type Value = Input;
    type Output = api::TemplateImportReply;
    fn declaration(_input: &Input) -> &'static Declaration<Role> {
        &declared::IMPORT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let input = match ctx.optional_retained_source::<Input>()? {
                Some(input) => (*input).clone(),
                None => select(ctx, conn, caller, body).await?,
            };
            let (_, facts) = Proposal::prepare(conn, caller, &input, Time::new(ctx.now))
                .await
                .map_err(source_error)?;
            let plan = native::bind_required(ctx, &declared::IMPORT, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                Proposal::locks(conn, &principal, &input, &input)
                    .await
                    .map_err(source_error)?;
            }
            Proposal::prepare(conn, caller, &input, Time::new(ctx.now))
                .await
                .map_err(source_error)
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            Proposal::require(conn, principal, input, input)
                .await
                .map_err(source_error)
        })
    }
}

fn source_error(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Refused | continuing::Error::Binding(_) => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original template proposal source is unavailable",
        ),
        continuing::Error::Native(error) => ApiError::engine(error),
    }
}

fn unavailable() -> ApiError {
    ApiError::new(
        ErrorCode::SourceOffline,
        "the host could not propose this template",
    )
}

async fn failed(ctx: &Ctx, error: ApiError) -> ApiError {
    let current = async {
        let mut tx = ctx.app.db.begin().await?;
        let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
        guard.check_deadline(&current)?;
        tx.rollback().await.map_err(DbError::from)?;
        Ok::<(), ApiError>(())
    }
    .await;
    current.err().unwrap_or(error)
}

impl effects::ApplyOwned for Import {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        input: &'a Input,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            ctx.retain_source(input.clone())?;
            let original = before.deadline();
            let identity =
                transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read)
                    .await?;
            let session = match &identity.user.via {
                Via::Session { session_id } => Some(*session_id),
                Via::Token { .. } | Via::OwnerRead { .. } => None,
            };
            let command = Outgoing::new(
                orch_types::new_id(),
                input.machine(),
                SerialKey::Machine,
                input.command(),
            )
            .map_err(ApiError::internal)?;
            let authority = native_imports::Command::capture(
                &mut tx,
                principal.caller(),
                session,
                identity.expires_at,
                input.clone(),
                &command,
                Time::new(ctx.now),
            )
            .await
            .map_err(ApiError::engine)?;
            tx.rollback().await.map_err(DbError::from)?;
            for archive in archive_references(&input.command().values)
                .map_err(|_| ApiError::internal("the native proposal has invalid archive fields"))?
            {
                if !ctx
                    .app
                    .blobs
                    .store()
                    .exists(&archive.hash)
                    .await
                    .map_err(|_| ApiError::internal("the uploaded archive could not be checked"))?
                {
                    return Err(invalid(
                        &format!("values.{}", archive.name),
                        "this caller has no completed upload of that archive",
                    ));
                }
            }
            if ctx
                .app
                .link
                .dispatcher()
                .issue_stored_before(
                    StoredCommand {
                        id: Some(command.id),
                        machine: command.machine,
                        kind: command.kind.as_str().to_owned(),
                        key: command.key.wire(),
                        deadline: command.deadline,
                        body: command.body,
                        authority: Some(CommandAuthority::template_import(authority.clone())),
                    },
                    Box::new(move || {
                        before.check().map_err(|error| {
                            Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                        })
                    }),
                )
                .await
                .is_err()
            {
                return Err(failed(ctx, unavailable()).await);
            }
            let proposal: NodeTemplateImportResult = match ctx
                .app
                .link
                .dispatcher()
                .wait_for::<NodeTemplateImport>(command.id)
                .await
            {
                Ok(reply) => reply,
                Err(_) => return Err(failed(ctx, unavailable()).await),
            };
            let mut tx = ctx.app.db.begin().await?;
            let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
            let frame = continuing::prepare_with_caller::<Proposal>(
                &mut tx,
                ctx.app.clock.as_ref(),
                authority.origin(),
                guard.principal().caller(),
                input.clone(),
            )
            .await
            .map_err(source_error)?;
            frame
                .into_parts()
                .first_effect
                .check(ctx.app.clock.as_ref())
                .map_err(source_error)?;
            clean_spec("spec", &proposal.spec)?;
            super::super::validation::value("spec", input.schema(), &proposal.spec)?;
            let result_hash =
                native_imports::Command::result_hash(&proposal).map_err(ApiError::engine)?;
            original.check()?;
            guard.check_deadline(&current)?;
            tx.commit().await.map_err(DbError::from)?;
            original.check()?;
            publication::retain(
                ctx,
                principal.caller(),
                (authority, result_hash),
                |ctx, conn, current, source| {
                    Box::pin(async move {
                        if source
                            .0
                            .current_completed(
                                conn,
                                current.caller(),
                                ctx.app.clock.as_ref(),
                                source.1,
                            )
                            .await
                            .map_err(ApiError::engine)?
                            .is_none()
                        {
                            return Err(ApiError::new(
                                ErrorCode::AuthzDenied,
                                "the original template proposal source is unavailable",
                            ));
                        }
                        Ok(())
                    })
                },
            )?;
            Ok(effects::Outcome::Applied(reply(proposal)))
        })
    }
}

fn reply(proposal: NodeTemplateImportResult) -> api::TemplateImportReply {
    api::TemplateImportReply {
        spec: proposal.spec,
        notes: proposal
            .notes
            .into_iter()
            .map(|note| api::TemplateImportNote {
                level: match note.level {
                    NoteLevel::Info => api::TemplateImportNoteLevel::Info,
                    NoteLevel::Warning => api::TemplateImportNoteLevel::Warning,
                    NoteLevel::Blocker => api::TemplateImportNoteLevel::Blocker,
                },
                text: note.text,
            })
            .collect(),
    }
}

pub(super) async fn import(
    ctx: Ctx,
    (): (),
    _body: api::TemplateImportBody,
) -> ApiResult<api::TemplateImportReply> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    Ok(effects::apply_owned::<Import>(&ctx, tx).await?.output)
}
