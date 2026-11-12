//! Named native profiles replay current metadata through the shared optional Read factory.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::reply;

use crate::key_operations::targets;

use crate::middleware::idempotency::ObservedCreation;

use crate::middleware::idempotency::Registered;

use crate::middleware::idempotency::commands;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::interactive_creation_sources as sources;

use orch_api_contract::interactive_creation_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_db::idempotency::creation::Created;

use orch_link_contract::commands::BrowserProfileCommand;

use orch_link_contract::commands::CommandKind;
const KIND: &str = "browser_profile";

struct Input {
    created: Created,
    environment: Id,
    name: String,
    first: Option<BrowserProfile>,
}

struct Subject {
    site: rows::Site,
    profile: BrowserProfile,
}

struct Content;

pub(super) fn binding() -> Registered<(), BrowserProfileCreateBody, BrowserProfileCreateResponse> {
    Registered::from(ObservedCreation {
        kind: KIND,
        replay: |ctx, (), body: BrowserProfileCreateBody, created| {
            Box::pin(async move {
                let environment = ctx.id("id")?;
                if created.kind != KIND || created.data["environment_id"] != json!(environment) {
                    return Err(ApiError::internal(
                        "the accepted profile association changed",
                    ));
                }
                project(
                    &ctx,
                    None,
                    Input {
                        created,
                        environment,
                        name: body.name,
                        first: None,
                    },
                )
                .await
            })
        },
    })
    .command(commands::CreationCommand {
        kind: CommandKind::BrowserProfile,
        identity: |row| {
            let command: BrowserProfileCommand =
                serde_json::from_value(row.body.clone()).map_err(ApiError::internal)?;
            if command.action != BrowserProfileAction::Create || command.to.is_some() {
                return Err(ApiError::internal(
                    "the profile creation declaration saw a different native act",
                ));
            }
            Ok(Some(Created {
                kind: KIND.into(),
                id: row.id,
                data: json!({"environment_id":command.environment_id}),
            }))
        },
    })
}

pub(super) async fn first(
    ctx: &Ctx,
    accepted: &orch_authz::Caller,
    command: Id,
    profile: BrowserProfile,
) -> ApiResult<BrowserProfileCreateResponse> {
    let environment = ctx.id("id")?;
    project(
        ctx,
        Some(accepted),
        Input {
            created: Created {
                kind: KIND.into(),
                id: command,
                data: json!({"environment_id":environment}),
            },
            environment,
            name: profile.name.clone(),
            first: Some(profile),
        },
    )
    .await
}

async fn project(
    ctx: &Ctx,
    accepted: Option<&orch_authz::Caller>,
    input: Input,
) -> ApiResult<BrowserProfileCreateResponse> {
    let receipt = BrowserProfileCreateResponse::Receipt(BrowserProfileCreateReceipt {
        projection: ReceiptProjection::Receipt,
        command_id: input.created.id,
        environment_id: input.environment,
    });
    crate::key_operations::publication::receipt(ctx, &receipt)?;
    let full = if let Some(accepted) = accepted {
        reply::render::<Content>(ctx, accepted, input).await
    } else {
        reply::render_completed_replay::<Content>(ctx, ctx.user()?.id(), input).await
    };
    match full {
        Ok(Some(profile)) => Ok(BrowserProfileCreateResponse::Full(Box::new(profile))),
        Ok(None) => Ok(receipt),
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(receipt),
        Err(error) => Err(error),
    }
}

impl reply::Projection for Content {
    type Source = Source;
    type Input = Input;
    type Subject = Subject;
    type Value = ();
    type Reply = BrowserProfile;
    const READ_SOURCE: Source = Source::Environment;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &sources::PROFILE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if ctx.id("id")? != input.environment {
                return Err(ApiError::not_found());
            }
            if input.first.is_none() {
                let command = commands::load(ctx, conn, &input.created)
                    .await?
                    .filter(|row| row.id == input.created.id && row.state == "succeeded")
                    .ok_or_else(ApiError::not_found)?;
                let actual: BrowserProfileCommand =
                    serde_json::from_value(command.body).map_err(ApiError::internal)?;
                if actual.environment_id != input.environment
                    || actual.name != input.name
                    || actual.action != BrowserProfileAction::Create
                    || actual.to.is_some()
                {
                    return Err(ApiError::not_found());
                }
            }
            let site = rows::site(conn, input.environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let profile = match &input.first {
                Some(profile) => profile.clone(),
                None => {
                    // The same current native profile list the page reads; never the first ACK bytes.
                    if Browsers::off(conn, &site)
                        .await
                        .map_err(ApiError::internal)?
                        .is_some()
                    {
                        return Err(ApiError::not_found());
                    }
                    let profile = ctx
                        .app
                        .engine
                        .browsers()
                        .memory()
                        .profiles(input.environment)
                        .unwrap_or_default()
                        .into_iter()
                        .find(|profile| profile.name == input.name)
                        .ok_or_else(ApiError::not_found)?;
                    BrowserProfile {
                        name: profile.name,
                        size_bytes: profile.size_bytes,
                        used_at: profile.used_at,
                        browser: profile.browser,
                    }
                }
            };
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: site.id,
                },
            )
            .await?;
            Ok((
                Subject { site, profile },
                vec![native::Facts::one(
                    Source::Environment,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if input.first.is_none() {
                require(conn, principal, subject.site.id, Permission::BrowserView).await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<BrowserProfile>> {
        Box::pin(async move { Ok(opened.subject.profile.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.site.id == fresh.subject.site.id
                && old.subject.site.generation == fresh.subject.site.generation
                && old.subject.site.machine_id == fresh.subject.site.machine_id
                && old.subject.site.workspace_root == fresh.subject.site.workspace_root
                && old.subject.profile.name == fresh.subject.profile.name)
        })
    }
}
