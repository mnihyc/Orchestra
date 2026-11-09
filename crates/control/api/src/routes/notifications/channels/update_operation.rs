//! One native validation, fresh serialized channel operands and a typed optional readback.

use super::*;
use crate::key_operations::native::{self, effects, reply};
use crate::key_operations::{publication, targets};
use futures::future::BoxFuture;
use orch_api_contract::notification_sources::{self as sources, Source};
use orch_api_contract::notifications::{
    NotificationChannelUpdateReceipt, NotificationChannelUpdateResponse,
};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_authz::Caller;
use orch_notifier_contract::delivery::Config;
use orch_types::Hash;
use std::sync::Arc;

pub(crate) struct Update;
pub(crate) struct Initial {
    row: ChannelRow,
    body: NotificationChannelUpdateBody,
    name: Option<String>,
    overrides: BTreeMap<String, TemplateOverride>,
    templates: Json,
    current: Option<Config>,
}
pub(crate) struct Fetched {
    row: ChannelRow,
    body: NotificationChannelUpdateBody,
    name: Option<String>,
    templates: Json,
    checked: Option<Rechecked>,
    stamp: Option<Stamp>,
}
#[derive(PartialEq, Eq)]
struct Stamp {
    values: Option<Json>,
    secrets: BTreeMap<String, Hash>,
    allowed: Option<BTreeSet<String>>,
}
fn stamp(config: &Config, body: &NotificationChannelUpdateBody) -> Stamp {
    Stamp {
        values: body.config.is_none().then(|| config.values.clone()),
        secrets: config
            .secrets
            .iter()
            .filter(|(name, _)| {
                !body
                    .secrets
                    .as_ref()
                    .is_some_and(|given| given.contains_key(*name))
            })
            .map(|(name, value)| (name.clone(), Hash::of(value.expose().as_bytes())))
            .collect(),
        allowed: config
            .allowed_private_hosts
            .as_ref()
            .map(|hosts| hosts.iter().cloned().collect()),
    }
}
pub(crate) struct Value {
    row: ChannelRow,
    fetched: Arc<Fetched>,
}
pub(crate) struct Output {
    row: ChannelRow,
    operation: ContentOperation,
    replaying: bool,
}
#[derive(Clone, Copy)]
enum ContentOperation {
    Update,
    System,
    Org,
    Own,
}
impl ContentOperation {
    fn row(self) -> &'static Declaration<Source> {
        match self {
            Self::Update => &sources::CHANNEL_UPDATE,
            Self::System => &sources::CHANNEL_CREATE_SYSTEM,
            Self::Org => &sources::CHANNEL_CREATE_ORG,
            Self::Own => &sources::CHANNEL_CREATE_OWN,
        }
    }
    fn creation(ctx: &Ctx) -> ApiResult<Self> {
        match ctx.route.name {
            "AdminNotificationChannelCreate" => Ok(Self::System),
            "OrgNotificationChannelCreate" => Ok(Self::Org),
            "MeNotificationChannelCreate" => Ok(Self::Own),
            _ => Err(ApiError::internal(
                "the native channel creator changed kind",
            )),
        }
    }
    fn matches(self, ctx: &Ctx, row: &ChannelRow) -> ApiResult<bool> {
        Ok(match self {
            Self::System => row.org_id.is_none() && row.user_id.is_none(),
            Self::Org => row.org_id == Some(ctx.id("id")?) && row.user_id.is_none(),
            Self::Own => row.org_id.is_none() && row.user_id == Some(ctx.user()?.id()),
            Self::Update => true,
        })
    }
}
fn same_owner(old: &ChannelRow, current: &ChannelRow) -> bool {
    old.id == current.id
        && old.org_id == current.org_id
        && old.user_id == current.user_id
        && old.kind == current.kind
}
fn changed_during_check() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the channel changed while its settings were checked; send the change again",
    )
}
async fn facts(conn: &mut Conn, row: &ChannelRow) -> ApiResult<Vec<native::Facts<Source>>> {
    let (family, target) = operation::owner(conn, row).await?;
    Ok(vec![native::Facts::one(
        Source::ChannelOwner,
        family,
        target,
    )])
}
async fn early(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: &ChannelRow,
    facts: &[native::Facts<Source>],
) -> ApiResult<()> {
    let known = native::bind_known(ctx, &sources::CHANNEL_UPDATE, facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    operation::require_change_private(conn, &principal, row).await
}

impl effects::Initial for Update {
    type Input = Initial;
    type Fetched = Fetched;
    fn declaration(_input: &Initial) -> &'static Declaration<Source> {
        &sources::CHANNEL_UPDATE
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: NotificationChannelUpdateBody,
    ) -> BoxFuture<'a, ApiResult<(Initial, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = channel_of(conn, ctx).await?;
            let facts = facts(conn, &row).await?;
            early(ctx, conn, caller, &row, &facts).await?;
            let name = body.name.as_deref().map(checked_name).transpose()?;
            let (overrides, templates) = templates_of(body.templates.as_ref())?;
            let current = if body.config.is_some() || body.secrets.is_some() {
                Some(
                    config_of_channel(conn, master_key()?, &row)
                        .await
                        .map_err(internal)?,
                )
            } else {
                None
            };
            Ok((
                Initial {
                    row,
                    body,
                    name,
                    overrides,
                    templates,
                    current,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Initial,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(operation::require_change_private(
            conn, principal, &value.row,
        ))
    }
    fn fetch<'a>(
        _ctx: &'a Ctx,
        _caller: &'a Caller,
        value: Initial,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let stamp = value
                .current
                .as_ref()
                .map(|current| stamp(current, &value.body));
            let checked = if let Some(current) = &value.current {
                Some(
                    recheck(Recheck {
                        kind: &value.row.kind,
                        target: Target::Person,
                        stored: &value.row.config,
                        current,
                        config: value.body.config.as_ref(),
                        secrets: value.body.secrets.as_ref(),
                        texts: &value.overrides,
                        allowed: current.allowed_private_hosts.clone(),
                    })
                    .await?,
                )
            } else {
                if value.body.templates.is_some() {
                    check_texts_of(&value.row.kind, &value.overrides)?;
                }
                None
            };
            Ok(Fetched {
                row: value.row,
                body: value.body,
                name: value.name,
                templates: value.templates,
                checked,
                stamp,
            })
        })
    }
}

impl effects::Effect for Update {
    type Source = Source;
    type Query = ();
    type Body = NotificationChannelUpdateBody;
    type Value = Value;
    type Output = Output;
    fn declaration(_value: &Value) -> &'static Declaration<Source> {
        &sources::CHANNEL_UPDATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        _body: NotificationChannelUpdateBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Self>(ctx)?;
            let mut row = channel_of(conn, ctx).await?;
            if !same_owner(&fetched.row, &row) {
                return Err(changed_during_check());
            }
            let known = facts(conn, &row).await?;
            early(ctx, conn, caller, &row, &known).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                row = rows::lock_channel_for_write(conn, row.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if !same_owner(&fetched.row, &row) {
                    return Err(changed_during_check());
                }
            }
            if let Some(expected) = &fetched.stamp {
                let current = config_of_channel(conn, master_key()?, &row)
                    .await
                    .map_err(internal)?;
                if stamp(&current, &fetched.body) != *expected {
                    return Err(changed_during_check());
                }
            }
            let facts = facts(conn, &row).await?;
            Ok((Value { row, fetched }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(operation::require_change_private(
            conn, principal, &value.row,
        ))
    }
}

impl effects::Apply for Update {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Output>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let fetched = &value.fetched;
            let row = &value.row;
            let changed = rows::update_channel_before(
                conn,
                row.id,
                &ChannelChange {
                    name: fetched.name.as_deref(),
                    config: fetched.checked.as_ref().map(|checked| &checked.values),
                    templates: fetched.body.templates.as_ref().map(|_| &fetched.templates),
                    enabled: fetched.body.enabled,
                },
                ctx.now,
                || before.check().map_err(operation::MutationError::Before),
            )
            .await
            .map_err(operation::MutationError::into_api)?
            .ok_or_else(ApiError::not_found)?;
            let mut sealed_names = Vec::new();
            if let Some(checked) = &fetched.checked {
                match &checked.unreached {
                    Some(error) => rows::record_channel_error(conn, row.id, error).await?,
                    None => pages::clear_channel_error(conn, row.id).await?,
                }
                rows::set_link_cursor(conn, row.id, None).await?;
                for (name, value) in &checked.put {
                    let sealed = seal_secret(master_key()?, owner::CHANNEL, row.id, name, value)
                        .map_err(|_| internal("a secret could not be sealed"))?;
                    settings::put_secret(
                        conn,
                        owner::CHANNEL,
                        row.id,
                        name,
                        &sealed,
                        Some(principal.user()),
                        ctx.now,
                    )
                    .await?;
                    sealed_names.push(name.clone());
                }
                for name in &checked.remove {
                    settings::delete_secret(conn, owner::CHANNEL, row.id, name).await?;
                }
            }
            let maker = match (row.org_id, row.user_id) {
                (Some(org), _) => Maker::Org(org),
                (None, Some(person)) => Maker::Person(person),
                (None, None) => Maker::System,
            };
            if !matches!(maker, Maker::Person(_)) {
                write_audit(
                    conn,
                    ctx,
                    Audited {
                        action: orch_db::audit::action!(AuditAction::ChannelUpdated),
                        maker,
                        channel: row.id,
                        kind: &row.kind,
                        name: &changed.name,
                        values: &changed.config,
                        affected: None,
                        secrets: &sealed_names,
                    },
                )
                .await?;
            }
            let row = rows::channel(conn, row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Output {
                row,
                operation: ContentOperation::Update,
                replaying: false,
            }))
        })
    }
}

pub(crate) async fn run(
    ctx: Ctx,
    (): (),
    _body: NotificationChannelUpdateBody,
) -> ApiResult<NotificationChannelUpdateResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Update>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = NotificationChannelUpdateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        channel_id: completed.output.row.id,
    };
    let restricted = NotificationChannelUpdateResponse::Receipt(receipt);
    publication::receipt(&completed.ctx, &restricted)?;
    match reply::render::<Content>(&completed.ctx, &completed.accepted, completed.output).await? {
        Some(full) => Ok(NotificationChannelUpdateResponse::full(full)),
        None => Ok(restricted),
    }
}

struct Content;
struct ContentValue {
    private_routes: bool,
    can_edit: bool,
    route: Option<route_rows::RouteRow>,
    good_link: bool,
}
impl reply::Projection for Content {
    type Source = Source;
    type Input = Output;
    type Subject = ChannelRow;
    type Value = ContentValue;
    type Reply = NotificationChannel;
    const READ_SOURCE: Source = Source::ChannelOwner;
    fn declaration(input: &Output) -> &'static Declaration<Source> {
        input.operation.row()
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Output,
    ) -> BoxFuture<'a, ApiResult<(ChannelRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::channel(conn, input.row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if !same_owner(&input.row, &row) {
                return Err(ApiError::not_found());
            }
            let facts = facts(conn, &row).await?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Output,
        subject: &'a ChannelRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if input.replaying
                && !may_change(conn, principal, subject, &mut HashMap::new()).await?
                && !pages::channels_reaching(conn, principal.user())
                    .await?
                    .iter()
                    .any(|row| row.id == subject.id)
            {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Output,
        subject: &'a ChannelRow,
    ) -> BoxFuture<'a, ApiResult<(ContentValue, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let account = targets::from_account(conn, &ctx.user()?.user).await?;
            let declaration = input.operation.row();
            let plan = crate::registry::OperationPlan::new(ctx.route, declaration.operation)?;
            let private_routes = native::permits_usage(
                &plan,
                declaration,
                principal.caller(),
                &Source::ChannelCallerRoutes,
                FacetUsage::ResponseContent,
                KeyTargetKind::Identity(KeyIdentityKind::Account),
                &account,
            )?;
            let route = if private_routes {
                pages::routes_of(conn, principal.user())
                    .await?
                    .into_iter()
                    .find(|route| route.channel_id == Some(subject.id))
            } else {
                None
            };
            let good_link = route.as_ref().is_some_and(|route| {
                route.link_hash.is_some()
                    && route
                        .link_expires_at
                        .is_some_and(|end| end > ctx.app.clock.now())
            });
            let can_edit = may_change(conn, principal, subject, &mut HashMap::new()).await?;
            let actor = native::Facts::collection(
                Source::ChannelCallerRoutes,
                if private_routes {
                    vec![(
                        KeyTargetKind::Identity(KeyIdentityKind::Account),
                        vec![account],
                    )]
                } else {
                    Vec::new()
                },
            );
            Ok((
                ContentValue {
                    private_routes,
                    can_edit,
                    route,
                    good_link,
                },
                vec![actor],
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Output,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<NotificationChannel>> {
        Box::pin(async move {
            prepare_views(
                conn,
                principal,
                std::slice::from_ref(&value.subject),
                value.value.private_routes,
            )
            .await?
            .render()
            .into_iter()
            .next()
            .ok_or_else(ApiError::not_found)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Output,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(same_owner(&old.subject, &fresh.subject)
                && (!old.value.can_edit || fresh.value.can_edit)
                && (!old.value.private_routes || fresh.value.private_routes)
                && old.value.route.as_ref().is_none_or(|route| {
                    fresh.value.route.as_ref().is_some_and(|current| {
                        route.id == current.id
                            && route.user_id == current.user_id
                            && route.channel_id == current.channel_id
                            && (!old.value.good_link
                                || (fresh.value.good_link
                                    && route.link_hash == current.link_hash
                                    && route.link_expires_at == current.link_expires_at))
                    })
                }))
        })
    }
}

/// All creator namespaces reuse the existing native channel renderer and its private-route cut.
pub(crate) fn creation<B: Send + 'static>()
-> crate::middleware::idempotency::Creation<(), B, NotificationChannelUpdateResponse> {
    crate::middleware::idempotency::Creation {
        kind: "notification_channel",
        replay: replay::<B>,
        identity: |value| {
            let id = match value {
                NotificationChannelUpdateResponse::Full(row) => row.id,
                NotificationChannelUpdateResponse::Receipt(row) => row.channel_id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "notification_channel".into(),
                id,
                data: Json::Null,
            })
        },
    }
}
fn receipt(id: Id) -> NotificationChannelUpdateResponse {
    NotificationChannelUpdateResponse::Receipt(NotificationChannelUpdateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        channel_id: id,
    })
}
pub(super) async fn created(
    ctx: &Ctx,
    accepted: &Caller,
    row: ChannelRow,
) -> ApiResult<NotificationChannelUpdateResponse> {
    let restricted = receipt(row.id);
    publication::receipt(ctx, &restricted)?;
    let operation = ContentOperation::creation(ctx)?;
    Ok(
        match reply::render::<Content>(
            ctx,
            accepted,
            Output {
                row,
                operation,
                replaying: false,
            },
        )
        .await?
        {
            Some(full) => NotificationChannelUpdateResponse::full(full),
            None => restricted,
        },
    )
}
fn replay<B: Send + 'static>(
    ctx: Ctx,
    (): (),
    _body: B,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<NotificationChannelUpdateResponse>> {
    Box::pin(async move {
        let restricted = receipt(created.id);
        publication::receipt(&ctx, &restricted)?;
        let result = async {
            let operation = ContentOperation::creation(&ctx)?;
            let row = rows::channel(&mut *ctx.app.db.acquire().await?, created.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if !operation.matches(&ctx, &row)? {
                return Err(ApiError::not_found());
            }
            reply::render_completed_replay::<Content>(
                &ctx,
                ctx.user()?.id(),
                Output {
                    row,
                    operation,
                    replaying: true,
                },
            )
            .await
        }
        .await;
        match result {
            Ok(Some(full)) => Ok(NotificationChannelUpdateResponse::full(full)),
            Ok(None) => Ok(restricted),
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(restricted),
            Err(error) => Err(error),
        }
    })
}
