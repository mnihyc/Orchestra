//! Native browser/display creation results use one current session projection.

use super::browsers;

use super::displays;

use super::interactive_close;

use crate::key_operations::native;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::access::page::NameBounds;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::browsers::Browser;

use orch_api_contract::browsers::BrowserCreateReceipt;

use orch_api_contract::browsers::BrowserCreateResponse;

use orch_api_contract::browsers::EnvironmentBrowserCreateBody;

use orch_api_contract::displays::Display;

use orch_api_contract::displays::DisplayCreateReceipt;

use orch_api_contract::displays::DisplayCreateResponse;

use orch_api_contract::displays::EnvironmentDisplayCreateBody;

use orch_api_contract::environment::ThroughThreadQuery;

use orch_api_contract::interactive_creation_sources as sources;

use orch_api_contract::interactive_creation_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::browsers as browser_rows;

use orch_db::queries::displays as display_rows;

use orch_types::Id;
#[derive(Clone, Copy)]
enum Kind {
    Browser,
    Display,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Factual {
    environment: Id,
    generation: i32,
    thread: Option<Id>,
    opened_by: Option<Id>,
}

struct Input {
    id: Id,
    kind: Kind,
    environment: Id,
    first: bool,
}

struct View {
    names: NameBounds,
    manages: bool,
}

enum Projected {
    Browser(Browser),
    Display(Display),
}

struct Content;

/// Creation has a stored Environment; optional session/thread reads follow the actual result.
pub(super) struct Creation;

impl native::Stored for Creation {
    type Source = Source;
    type Value = (Id, Permission);
    const ROWS: &'static [Declaration<Source>] = &[sources::BROWSER, sources::DISPLAY];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Source>,
    ) -> native::SourceFuture<'a, Self::Value, Source> {
        Box::pin(async move {
            let kind = match declaration.operation {
                "EnvironmentBrowserCreate" => Kind::Browser,
                "EnvironmentDisplayCreate" => Kind::Display,
                _ => return Err(ApiError::internal("unknown interactive creation source")),
            };
            let environment = ctx.id("id")?;
            let site = browser_rows::site(conn, environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok((
                (site.id, kind.manage_permission()),
                facts(conn, site.id).await?,
            ))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { browsers::require(conn, principal, value.0, value.1).await })
    }
}

impl Kind {
    fn declaration(self) -> &'static Declaration<Source> {
        match self {
            Self::Browser => &sources::BROWSER,
            Self::Display => &sources::DISPLAY,
        }
    }
    fn view_permission(self) -> Permission {
        match self {
            Self::Browser => Permission::BrowserView,
            Self::Display => Permission::VncView,
        }
    }
    fn manage_permission(self) -> Permission {
        match self {
            Self::Browser => Permission::BrowserManage,
            Self::Display => Permission::VncManage,
        }
    }
}

fn browser_facts(row: &browser_rows::BrowserRow) -> Factual {
    Factual {
        environment: row.environment_id,
        generation: row.env_generation,
        thread: row.thread_id,
        opened_by: row.opened_by,
    }
}

fn display_facts(row: &display_rows::DisplayRow) -> Factual {
    Factual {
        environment: row.environment_id,
        generation: row.env_generation,
        thread: row.thread_id,
        opened_by: row.opened_by,
    }
}

async fn facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![native::Facts::one(
        Source::Environment,
        KeyTargetKind::Object(ObjectKind::Environment),
        targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await?,
    )])
}

impl reply::Projection for Content {
    type Source = Source;
    type Input = Input;
    type Subject = Factual;
    type Value = View;
    type Reply = Projected;
    const READ_SOURCE: Source = Source::Environment;
    fn declaration(input: &Input) -> &'static Declaration<Source> {
        input.kind.declaration()
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(Factual, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let current = match input.kind {
                Kind::Browser => browser_facts(
                    &browser_rows::get(conn, input.id)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                ),
                Kind::Display => display_facts(
                    &display_rows::get(conn, input.id)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                ),
            };
            if current.environment != input.environment || ctx.id("id")? != current.environment {
                return Err(ApiError::not_found());
            }
            Ok((current, facts(conn, current.environment).await?))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
        subject: &'a Factual,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            // The original own browser result never required a second View after
            // its management act. A new replay is its current ordinary read.
            if !input.first || matches!(input.kind, Kind::Display) {
                browsers::require(conn, p, subject.environment, input.kind.view_permission())
                    .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
        subject: &'a Factual,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (_, mut names) = interactive_close::plan(
                ctx,
                conn,
                p.caller(),
                subject.environment,
                subject.thread,
                false,
            )
            .await?;
            let visible = names
                .refs(conn, p, &subject.thread.into_iter().collect::<Vec<_>>())
                .await?;
            names
                .nodes
                .retain(|id| visible.get(id).is_some_and(|node| !node.redacted));
            let mut threads = Vec::new();
            for &id in &names.nodes {
                threads.push(targets::node_identity(conn, id).await?);
            }
            let labels = native::Facts::collection(
                Source::Threads,
                if threads.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Thread), threads)]
                },
            );
            let manages =
                browsers::holds(conn, p, subject.environment, input.kind.manage_permission())
                    .await?;
            Ok((View { names, manages }, vec![labels]))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Projected>> {
        Box::pin(async move {
            match input.kind {
                Kind::Browser => Ok(Projected::Browser(
                    browsers::browser_in(
                        &ctx.app,
                        conn,
                        p,
                        opened.subject.environment,
                        input.id,
                        Some(&opened.value.names),
                    )
                    .await?,
                )),
                Kind::Display => Ok(Projected::Display(
                    displays::display_in(
                        &ctx.app,
                        conn,
                        p,
                        opened.subject.environment,
                        input.id,
                        Some(&opened.value.names),
                    )
                    .await?,
                )),
            }
        })
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
            Ok(old.subject == fresh.subject
                && (!old.value.manages || fresh.value.manages)
                && old.value.names.nodes.is_subset(&fresh.value.names.nodes))
        })
    }
}

fn browser_receipt(id: Id, environment: Id) -> BrowserCreateResponse {
    BrowserCreateResponse::Receipt(BrowserCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        browser_id: id,
        environment_id: environment,
    })
}

fn display_receipt(id: Id, environment: Id) -> DisplayCreateResponse {
    DisplayCreateResponse::Receipt(DisplayCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        display_id: id,
        environment_id: environment,
    })
}

async fn project(
    ctx: &Ctx,
    accepted: Option<&Caller>,
    input: Input,
) -> ApiResult<Option<Projected>> {
    let value = match accepted {
        Some(accepted) => reply::render::<Content>(ctx, accepted, input).await,
        None => reply::render_completed_replay::<Content>(ctx, ctx.user()?.id(), input).await,
    };
    match value {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(None),
        other => other,
    }
}

pub(super) async fn browser_created(
    ctx: &Ctx,
    accepted: &Caller,
    row: &browser_rows::BrowserRow,
) -> ApiResult<BrowserCreateResponse> {
    let restricted = browser_receipt(row.id, row.environment_id);
    publication::receipt(ctx, &restricted)?;
    Ok(
        match project(
            ctx,
            Some(accepted),
            Input {
                id: row.id,
                kind: Kind::Browser,
                environment: row.environment_id,
                first: true,
            },
        )
        .await?
        {
            Some(Projected::Browser(value)) => BrowserCreateResponse::full(value),
            None => restricted,
            Some(_) => {
                return Err(ApiError::internal(
                    "the created browser projection changed kind",
                ));
            }
        },
    )
}

pub(super) async fn display_created(
    ctx: &Ctx,
    accepted: &Caller,
    row: &display_rows::DisplayRow,
) -> ApiResult<DisplayCreateResponse> {
    let restricted = display_receipt(row.id, row.environment_id);
    publication::receipt(ctx, &restricted)?;
    Ok(
        match project(
            ctx,
            Some(accepted),
            Input {
                id: row.id,
                kind: Kind::Display,
                environment: row.environment_id,
                first: true,
            },
        )
        .await?
        {
            Some(Projected::Display(value)) => DisplayCreateResponse::full(value),
            None => restricted,
            Some(_) => {
                return Err(ApiError::internal(
                    "the created display projection changed kind",
                ));
            }
        },
    )
}

pub(super) fn browser_binding() -> crate::middleware::idempotency::Creation<
    ThroughThreadQuery,
    EnvironmentBrowserCreateBody,
    BrowserCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: "browser_creation",
        identity: |value| {
            let id = match value {
                BrowserCreateResponse::Full(value) => value.id,
                BrowserCreateResponse::Receipt(value) => value.browser_id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "browser_creation".into(),
                id,
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, _query, _body, created| {
            Box::pin(async move {
                let environment: Id =
                    serde_json::from_value(created.data.get("environment").cloned().ok_or_else(
                        || ApiError::internal("the accepted session has no native environment"),
                    )?)
                    .map_err(ApiError::internal)?;
                let restricted = browser_receipt(created.id, environment);
                publication::receipt(&ctx, &restricted)?;
                Ok(
                    match project(
                        &ctx,
                        None,
                        Input {
                            id: created.id,
                            kind: Kind::Browser,
                            environment,
                            first: false,
                        },
                    )
                    .await?
                    {
                        Some(Projected::Browser(value)) => BrowserCreateResponse::full(value),
                        None => restricted,
                        Some(_) => {
                            return Err(ApiError::internal(
                                "the browser replay projection changed kind",
                            ));
                        }
                    },
                )
            })
        },
    }
}

pub(super) fn display_binding() -> crate::middleware::idempotency::Creation<
    ThroughThreadQuery,
    EnvironmentDisplayCreateBody,
    DisplayCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: "display_creation",
        identity: |value| {
            let id = match value {
                DisplayCreateResponse::Full(value) => value.id,
                DisplayCreateResponse::Receipt(value) => value.display_id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "display_creation".into(),
                id,
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, _query, _body, created| {
            Box::pin(async move {
                let environment: Id =
                    serde_json::from_value(created.data.get("environment").cloned().ok_or_else(
                        || ApiError::internal("the accepted session has no native environment"),
                    )?)
                    .map_err(ApiError::internal)?;
                let restricted = display_receipt(created.id, environment);
                publication::receipt(&ctx, &restricted)?;
                Ok(
                    match project(
                        &ctx,
                        None,
                        Input {
                            id: created.id,
                            kind: Kind::Display,
                            environment,
                            first: false,
                        },
                    )
                    .await?
                    {
                        Some(Projected::Display(value)) => DisplayCreateResponse::full(value),
                        None => restricted,
                        Some(_) => {
                            return Err(ApiError::internal(
                                "the display replay projection changed kind",
                            ));
                        }
                    },
                )
            })
        },
    }
}
