//! Native Group and service Account creation with guarded operands and typed optional content.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::transaction_guard;

use crate::routes::access::page::NameBounds;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::directory::GroupCreateReceipt;

use orch_api_contract::directory::GroupCreateResponse;

use orch_api_contract::directory::OrgServiceAccountCreateBody;

use orch_api_contract::directory::ServiceAccount;

use orch_api_contract::directory::ServiceAccountCreateReceipt;

use orch_api_contract::directory::ServiceAccountCreateResponse;

use orch_api_contract::directory_sources as sources;

use orch_api_contract::directory_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use serde::Serialize;
const GROUP: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Group);

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

pub(super) struct Groups;

pub(super) struct Services;

pub(super) struct GroupInput {
    org: Placed,
    name: String,
    description: String,
}

pub(super) struct ServiceInput {
    org: Placed,
    username: String,
    display: String,
}

pub(super) struct GroupContent {
    row: access::GroupFull,
    names: NameBounds,
}

enum CreationError {
    Database(DbError),
    Before(ApiError),
}

impl From<DbError> for CreationError {
    fn from(error: DbError) -> Self {
        Self::Database(error)
    }
}

pub(super) trait Creation:
    effects::Apply<Source = Source, Query = (), Output = Id>
    + reply::Projection<Source = Source, Input = Id>
{
    type Response: Serialize + Send + 'static;
    const REPLAY_KIND: &'static str;
    fn identity(value: &Self::Response) -> Id;
    fn receipt(id: Id) -> Self::Response;
    fn full(value: <Self as reply::Projection>::Reply) -> Self::Response;
}

pub(super) fn replay_binding<F: Creation>()
-> crate::middleware::idempotency::Creation<(), <F as effects::Effect>::Body, F::Response> {
    crate::middleware::idempotency::Creation {
        kind: F::REPLAY_KIND,
        replay: replay::<F>,
        identity: |value| {
            Ok(orch_db::idempotency::creation::Created {
                kind: F::REPLAY_KIND.into(),
                id: F::identity(value),
                data: serde_json::Value::Null,
            })
        },
    }
}

fn replay<F: Creation>(
    ctx: Ctx,
    (): (),
    _body: <F as effects::Effect>::Body,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<F::Response>> {
    Box::pin(async move {
        let receipt = F::receipt(created.id);
        publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<F>(&ctx, ctx.user()?.id(), created.id).await {
            Ok(Some(value)) => Ok(F::full(value)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}

async fn keeper_facts(
    ctx: &Ctx,
    conn: &mut Conn,
    family: KeyTargetKind,
) -> ApiResult<(Placed, Vec<native::Facts<Source>>)> {
    let org = the_org(conn, ctx.id("id")?).await?;
    let target = targets::node(conn, org.id).await?;
    Ok((
        org,
        vec![native::Facts::one(Source::Organization, family, target)],
    ))
}

async fn require_keeper(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    org: Id,
) -> ApiResult<()> {
    require_on_keeper_projected(ctx, conn, principal, org, exact(Permission::MembersManage)).await
}

async fn preliminary_keeper(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    family: KeyTargetKind,
) -> ApiResult<(Placed, Vec<native::Facts<Source>>)> {
    let (org, facts) = keeper_facts(ctx, conn, family).await?;
    let known = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require_keeper(ctx, conn, &principal, org.id).await?;
    Ok((org, facts))
}

/// Preserve the ordinary early gate before contending for the native creation parent.
/// The actual guarded effect then resolves authority and typed operands after this wait.
pub(super) async fn run<F: Creation>(
    ctx: Ctx,
    (): (),
    _body: <F as effects::Effect>::Body,
) -> ApiResult<F::Response> {
    let mut tx = ctx.app.db.begin().await?;
    let early = ctx.operation_principal(&mut tx).await?;
    let org = the_org(&mut tx, ctx.id("id")?).await?;
    require_keeper(&ctx, &mut tx, &early, org.id).await?;
    let locked: Option<Id> =
        sqlx::query_scalar("SELECT id FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
            .bind(org.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?;
    if locked.is_none() {
        return Err(ApiError::not_found());
    }
    let completed = effects::apply::<F>(&ctx, &mut tx).await?;
    orch_db::idempotency::creation::observe(
        &mut tx,
        Some(ctx.user()?.id()),
        F::REPLAY_KIND,
        completed.output,
    )
    .await?;
    // These native SQL effects remain rollbackable. Recheck the complete current
    // admission after any insert/audit wait and before their actual commit.
    let (ctx, guard) = transaction_guard::after_wait(&completed.ctx, &mut tx).await?;
    guard.check_deadline(&ctx)?;
    let accepted = guard.principal().caller().clone();
    tx.commit().await.map_err(DbError::from)?;
    let receipt = F::receipt(completed.output);
    publication::receipt(&ctx, &receipt)?;
    Ok(
        match reply::render::<F>(&ctx, &accepted, completed.output).await? {
            Some(value) => F::full(value),
            None => receipt,
        },
    )
}

impl effects::Effect for Groups {
    type Source = Source;
    type Query = ();
    type Body = OrgGroupCreateBody;
    type Value = GroupInput;
    type Output = Id;

    fn declaration(_value: &GroupInput) -> &'static Declaration<Source> {
        &sources::GROUP_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: OrgGroupCreateBody,
    ) -> BoxFuture<'a, ApiResult<(GroupInput, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (org, facts) =
                preliminary_keeper(ctx, conn, caller, &sources::GROUP_CREATE, GROUP).await?;
            let name = clean_group_name(&body.name)?;
            let description = body.description.unwrap_or_default().trim().to_owned();
            Ok((
                GroupInput {
                    org,
                    name,
                    description,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a GroupInput,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_keeper(ctx, conn, principal, value.org.id))
    }
}

impl effects::Apply for Groups {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a GroupInput,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        Box::pin(async move {
            let id = new_id();
            access::create_group_before(
                conn,
                id,
                value.org.id,
                &value.name,
                &value.description,
                principal.user(),
                ctx.now,
                move || before.check().map_err(CreationError::Before),
            )
            .await
            .map_err(|error| match error {
                CreationError::Database(error) => group_taken(error),
                CreationError::Before(error) => error,
            })?;
            record_about(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::GroupCreated),
                id,
                value.org,
                &format!("{} made the group {}", actor_name(ctx)?, value.name),
                json!({"group":id,"name":value.name}),
            )
            .await?;
            Ok(effects::Outcome::Applied(id))
        })
    }
}

impl Creation for Groups {
    type Response = GroupCreateResponse;
    const REPLAY_KIND: &'static str = "group";
    fn identity(value: &Self::Response) -> Id {
        match value {
            GroupCreateResponse::Full(value) => value.id,
            GroupCreateResponse::Receipt(value) => value.group_id,
        }
    }
    fn receipt(id: Id) -> GroupCreateResponse {
        GroupCreateResponse::Receipt(GroupCreateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            group_id: id,
        })
    }
    fn full(value: Group) -> GroupCreateResponse {
        GroupCreateResponse::full(value)
    }
}

impl reply::Projection for Groups {
    type Source = Source;
    type Input = Id;
    type Subject = access::GroupRow;
    type Value = GroupContent;
    type Reply = Group;
    const READ_SOURCE: Source = Source::CreatedGroup;
    fn declaration(_input: &Id) -> &'static Declaration<Source> {
        &sources::GROUP_CREATE
    }

    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(access::GroupRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = access::groups(conn, &[*id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let (_, mut facts) = keeper_facts(ctx, conn, GROUP).await?;
            facts.push(native::Facts::one(
                Source::CreatedGroup,
                GROUP,
                targets::from_group_row(conn, &row).await?,
            ));
            Ok((row, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Id,
        subject: &'a access::GroupRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_keeper(ctx, conn, principal, subject.org_id))
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Id,
        subject: &'a access::GroupRow,
    ) -> BoxFuture<'a, ApiResult<(GroupContent, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = access::groups_full(conn, &[subject.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let graph = orch_authz::engine::Graph::load(conn, &[row.org_id])
                .await
                .map_err(ApiError::internal)?;
            let ids: Vec<_> = graph.nodes().map(|node| node.id).collect();
            let visible = crate::routes::nodes::view::reference_visibility(
                conn,
                principal,
                &ids,
                Policy::default(),
            )
            .await?;
            let plan =
                crate::registry::OperationPlan::new(ctx.route, sources::GROUP_CREATE.operation)?;
            let (facts, nodes) = native::node_name_facts(
                &plan,
                &sources::GROUP_CREATE,
                principal.caller(),
                &graph,
                &visible,
                &[
                    (ObjectKind::Org, Source::GroupOrgNames),
                    (ObjectKind::Project, Source::GroupProjectNames),
                    (ObjectKind::Topic, Source::GroupTopicNames),
                    (ObjectKind::Thread, Source::GroupThreadNames),
                ],
            )?;
            Ok((
                GroupContent {
                    row,
                    names: NameBounds {
                        limited: native::modern(principal.caller()),
                        nodes,
                        ..NameBounds::default()
                    },
                },
                facts,
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Id,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Group>> {
        Box::pin(async move {
            groups_named_bounded(
                conn,
                principal,
                vec![prepared.value.row.clone()],
                Some(&prepared.value.names),
            )
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Id,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.id == fresh.subject.id
                && opened.subject.org_id == fresh.subject.org_id
                && opened.subject.root_id == fresh.subject.root_id
                && opened.value.names.nodes.is_subset(&fresh.value.names.nodes))
        })
    }
}

impl effects::Effect for Services {
    type Source = Source;
    type Query = ();
    type Body = OrgServiceAccountCreateBody;
    type Value = ServiceInput;
    type Output = Id;
    fn declaration(_value: &ServiceInput) -> &'static Declaration<Source> {
        &sources::SERVICE_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: OrgServiceAccountCreateBody,
    ) -> BoxFuture<'a, ApiResult<(ServiceInput, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (org, facts) =
                preliminary_keeper(ctx, conn, caller, &sources::SERVICE_CREATE, ACCOUNT).await?;
            let username = clean_username(&body.username)?;
            let display = body.display_name.trim().to_owned();
            if display.is_empty() || display.chars().count() > SERVICE_NAME_MAX_CHARS {
                return Err(invalid(
                    "display_name",
                    &format!("must be 1 to {SERVICE_NAME_MAX_CHARS} characters"),
                ));
            }
            Ok((
                ServiceInput {
                    org,
                    username,
                    display,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a ServiceInput,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_keeper(ctx, conn, principal, value.org.id))
    }
}

impl effects::Apply for Services {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a ServiceInput,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        Box::pin(async move {
            let id = new_id();
            identity::create_service_account_before(
                conn,
                id,
                value.org.id,
                &value.username,
                &value.display,
                principal.user(),
                ctx.now,
                move || before.check().map_err(CreationError::Before),
            )
            .await
            .map_err(|error| match error {
                CreationError::Database(DbError::Unique { .. }) => {
                    ApiError::new(ErrorCode::ConflictExists, "that username is taken")
                }
                CreationError::Database(error) => error.into(),
                CreationError::Before(error) => error,
            })?;
            record_about(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::ServiceAccountCreated),
                id,
                value.org,
                &format!(
                    "{} made the service identity {}",
                    actor_name(ctx)?,
                    value.username
                ),
                json!({"user":id,"username":value.username}),
            )
            .await?;
            Ok(effects::Outcome::Applied(id))
        })
    }
}

impl Creation for Services {
    type Response = ServiceAccountCreateResponse;
    const REPLAY_KIND: &'static str = "service_account";
    fn identity(value: &Self::Response) -> Id {
        match value {
            ServiceAccountCreateResponse::Full(value) => value.user.id,
            ServiceAccountCreateResponse::Receipt(value) => value.service_account_id,
        }
    }
    fn receipt(id: Id) -> ServiceAccountCreateResponse {
        ServiceAccountCreateResponse::Receipt(ServiceAccountCreateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            service_account_id: id,
        })
    }
    fn full(value: ServiceAccount) -> ServiceAccountCreateResponse {
        ServiceAccountCreateResponse::full(value)
    }
}

impl reply::Projection for Services {
    type Source = Source;
    type Input = Id;
    type Subject = identity::UserRow;
    type Value = identity::ServiceRow;
    type Reply = ServiceAccount;
    const READ_SOURCE: Source = Source::CreatedService;
    fn declaration(_input: &Id) -> &'static Declaration<Source> {
        &sources::SERVICE_CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(identity::UserRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = identity::user_by_id(conn, *id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.kind != "service" {
                return Err(ApiError::not_found());
            }
            let (_, mut facts) = keeper_facts(ctx, conn, ACCOUNT).await?;
            facts.push(native::Facts::one(
                Source::CreatedService,
                ACCOUNT,
                targets::from_account(conn, &row).await?,
            ));
            Ok((row, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Id,
        subject: &'a identity::UserRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let org = subject.service_org_id.ok_or_else(ApiError::not_found)?;
            require_keeper(ctx, conn, principal, org).await
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Id,
        _subject: &'a identity::UserRow,
    ) -> BoxFuture<'a, ApiResult<(identity::ServiceRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = identity::service_account(conn, *input)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok((row, Vec::new()))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Id,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ServiceAccount>> {
        Box::pin(async move {
            services_shown(conn, std::slice::from_ref(&prepared.value))
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Id,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.id == fresh.subject.id
                && opened.subject.service_org_id == fresh.subject.service_org_id
                && opened.value.org_id == fresh.value.org_id)
        })
    }
}
