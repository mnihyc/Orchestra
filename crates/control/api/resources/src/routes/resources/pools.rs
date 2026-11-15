//! Pools use ordinary resource reads and the native guarded metadata/graph writer.
use super::{Registry, require};
use crate::key_operations::{native, native::effects};
use crate::middleware::{
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::Ctx;
use crate::routes::nodes::{invalid, placed};
use crate::routes::resources::{projection, read};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission, ResourceKind},
};
use orch_api_contract::{
    access::NewResourceAccess,
    common::{ErrorCode, ReceiptProjection},
    operation_sources::Declaration,
    pools as api,
};
use orch_authz::{
    Caller, Policy, Principal,
    ownership::{self, Making, NewAccess},
    resources::{self, Seen},
};
use orch_db::{
    Conn, DbError,
    queries::pools::{self as rows, Row},
};
use orch_provider_contract::pools::{Definition, MemberRule};
use orch_types::{Id, Json, Time, new_id};

const POOL: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Pool);
struct Pools;
impl read::Family for Pools {
    type Item = api::Pool;
    type ListQuery = api::PoolListQuery;
    const SPEC: &'static orch_api_contract::resource_reads::Family = &api::READS;
    const CARDINALITY: read::Cardinality = read::Cardinality::Exhaustive;
    fn require_get<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Pool,
            id,
            Permission::PoolView,
        ))
    }
    fn prepare_get<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<projection::Prepared>> {
        Box::pin(async move {
            let object = ObjectRef {
                kind: ObjectKind::Pool,
                id,
            };
            let (_, resource, _) = resources::site(conn, object)
                .await
                .map_err(ApiError::internal)?;
            let held = resources::effective(conn, principal, object, Policy::default())
                .await
                .map_err(ApiError::internal)?;
            let seen = Seen {
                resource,
                admin_place: held.admin_place,
                admin_only: held.admin_only,
                admin_access: held.admin_access,
                participating: held.participating,
                owner: held.owner,
                administers: held.administers,
                held: held.held,
            };
            projection::prepare(
                ctx,
                conn,
                principal,
                projection::Selection {
                    kind: ResourceKind::Pool,
                    ids: vec![id],
                    seen: &[seen],
                },
            )
            .await?
            .ok_or_else(ApiError::not_found)
        })
    }
    fn filter<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        mut seen: Vec<Seen>,
        query: &'a api::PoolListQuery,
    ) -> BoxFuture<'a, ApiResult<Vec<Seen>>> {
        Box::pin(async move {
            let ids: Vec<_> = seen.iter().map(|row| row.resource.id).collect();
            let rows = rows::among(conn, &ids).await?;
            let wanted = query
                .query
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            seen.retain(|seen| {
                rows.iter().any(|row| {
                    row.id == seen.resource.id
                        && query
                            .org_id
                            .is_none_or(|org| row.org_id == org || row.home_id == org)
                        && query.kind.is_none_or(|kind| row.kind == kind.as_str())
                        && (wanted.is_empty() || row.name.to_lowercase().contains(&wanted))
                })
            });
            Ok(seen)
        })
    }
    fn into_items(rendered: projection::Rendered) -> ApiResult<Vec<api::Pool>> {
        match rendered {
            projection::Rendered::Pool(rows) => Ok(rows),
            _ => Err(ApiError::internal(
                "the native pool read returned another resource family",
            )),
        }
    }
}

pub fn mount(registry: &mut Registry) {
    lending::mount(registry);
    read::register::<Pools, _, _>(registry, api::PoolGet, api::PoolList);
    registry.on_snapshot_read_with_optional_operation(
        api::PoolMembersGet,
        native::selected::<Members>,
        native::selected_read::<Members>,
    );
    registry
        .on_with_operation(api::PoolCreate, effects::prepare::<Create>, create)
        .creation(
            api::PoolCreate,
            crate::middleware::idempotency::Creation {
                kind: "pool",
                identity: |receipt: &api::PoolReceipt| {
                    Ok(orch_db::idempotency::creation::Created {
                        kind: "pool".into(),
                        id: receipt.id,
                        data: serde_json::json!({"revision":receipt.revision}),
                    })
                },
                replay: |ctx, (), _, created| {
                    Box::pin(async move {
                        if created.kind != "pool" {
                            return Err(ApiError::internal("the accepted resource kind changed"));
                        }
                        let receipt = api::PoolReceipt {
                            projection: ReceiptProjection::Receipt,
                            id: created.id,
                            revision: created
                                .data
                                .get("revision")
                                .and_then(Json::as_i64)
                                .ok_or_else(|| {
                                    ApiError::internal("the accepted pool receipt has no revision")
                                })?,
                        };
                        crate::key_operations::publication::receipt(&ctx, &receipt)?;
                        Ok(receipt)
                    })
                },
            },
        );
    registry.on_with_operation(api::PoolUpdate, effects::prepare::<Update>, update);
    registry.on_with_operation(
        api::PoolMemberPut,
        effects::prepare::<PutMember>,
        put_member,
    );
}

fn name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid("name", "a pool needs a name"));
    }
    Ok(value.into())
}
fn definition(value: &Definition) -> ApiResult<()> {
    value
        .validate()
        .map_err(|why| invalid("definition", &why))?;
    let faults = orch_settings::validation::check_own_values(
        orch_settings_contract::registry::Scope::Pool,
        &value.config,
    );
    if !faults.is_empty() {
        return Err(
            invalid("config", "the pool has invalid setting overrides").with("errors", faults)
        );
    }
    Ok(())
}
async fn home(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Id> {
    let (place, kind) = placed(conn, id).await?;
    if !matches!(
        kind,
        orch_api_contract::nodes::NodeKind::Org | orch_api_contract::nodes::NodeKind::Project
    ) {
        return Err(invalid(
            "home_id",
            "a pool is at home in an organization or project",
        ));
    }
    match ownership::may_make_here(conn, principal, ResourceKind::Pool, Some(id))
        .await
        .map_err(ApiError::internal)?
    {
        Making::Allowed(_) => Ok(place.org),
        Making::Refused(why) => Err(super::providers::refused_making(conn, id, why).await),
    }
}
async fn config_unlocked(conn: &mut Conn, home: Id, config: &Json) -> ApiResult<()> {
    for key in config.as_object().into_iter().flatten().map(|(key, _)| key) {
        let current = orch_settings::resolve(conn, orch_settings::Place::Node(home), key)
            .await
            .map_err(ApiError::internal)?;
        if current.resolved.locked_by.is_some() {
            return Err(invalid(key, "this pool default is locked by its home"));
        }
    }
    Ok(())
}
async fn facts(conn: &mut Conn, id: Id) -> ApiResult<Vec<native::Facts<api::Source>>> {
    let target = crate::key_operations::targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Pool,
            id,
        },
    )
    .await?;
    Ok(vec![native::Facts::one(api::Source::Subject, POOL, target)])
}
async fn pin(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<api::Source>,
) -> ApiResult<(Principal, Row)> {
    let id = ctx.id("id")?;
    let known = native::bind_known(ctx, declaration, &facts(conn, id).await?)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(
        conn,
        &principal,
        ResourceKind::Pool,
        id,
        Permission::PoolManage,
    )
    .await?;
    rows::lock_graph(conn).await?;
    let row = rows::lock(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let known = native::bind_known(ctx, declaration, &facts(conn, id).await?)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(
        conn,
        &principal,
        ResourceKind::Pool,
        id,
        Permission::PoolManage,
    )
    .await?;
    Ok((principal, row))
}
fn stale() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the pool changed since it was read",
    )
}
fn receipt(id: Id, revision: i64) -> api::PoolReceipt {
    api::PoolReceipt {
        projection: ReceiptProjection::Receipt,
        id,
        revision,
    }
}
async fn record(ctx: &Ctx, conn: &mut Conn, row: &Row, action: &str) -> ApiResult<()> {
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(ctx.user()?.id()),
        action,
        "pool",
        Some(row.id),
        "Changed the pool",
    );
    entry.node_id = Some(row.home_id);
    entry.org_id = Some(row.org_id);
    entry.history = true;
    entry.details = serde_json::json!({"revision":row.revision});
    orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
    Ok(())
}
fn member(row: orch_provider_contract::pools::Member, position: i64) -> ApiResult<rows::Member> {
    Ok(rows::Member {
        id: row.id,
        position,
        rule: serde_json::to_value(row.rule).map_err(ApiError::internal)?,
        enabled: row.enabled,
        priority: row.priority.map(i64::try_from).transpose().map_err(|_| {
            invalid(
                "member.priority",
                "the priority exceeds its stored integer representation",
            )
        })?,
        weight: row.weight,
    })
}
fn child(member: &rows::Member) -> ApiResult<Option<Id>> {
    let rule: MemberRule =
        serde_json::from_value(member.rule.clone()).map_err(ApiError::internal)?;
    Ok(match rule {
        MemberRule::Pool { id, .. } => Some(id),
        _ => None,
    })
}

struct Create;
struct Creation {
    row: Row,
    members: Vec<rows::Member>,
    access: NewAccess,
}
impl effects::Effect for Create {
    type Source = api::Source;
    type Query = ();
    type Body = api::PoolCreateBody;
    type Value = Creation;
    type Output = api::PoolReceipt;
    fn declaration(_value: &Creation) -> &'static Declaration<api::Source> {
        &api::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> native::SourceFuture<'a, Creation, api::Source> {
        Box::pin(async move {
            definition(&body.definition)?;
            let target = crate::key_operations::targets::node(conn, body.home_id).await?;
            let sources = vec![native::Facts::one(api::Source::Home, POOL, target)];
            let known = native::bind_known(ctx, &api::CREATE, &sources)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let org = home(conn, &principal, body.home_id).await?;
            rows::lock_graph(conn).await?;
            super::creation_home::lock(conn, Some(body.home_id)).await?;
            config_unlocked(conn, body.home_id, &body.definition.config).await?;
            let mut selected = body.definition;
            let members = std::mem::take(&mut selected.members)
                .into_iter()
                .enumerate()
                .map(|(position, row)| {
                    member(
                        row,
                        i64::try_from(position).map_err(|_| {
                            invalid(
                                "members",
                                "the member position exceeds its stored integer representation",
                            )
                        })?,
                    )
                })
                .collect::<ApiResult<Vec<_>>>()?;
            let row = Row {
                id: new_id(),
                org_id: org,
                home_id: body.home_id,
                owner_id: principal.user(),
                name: name(&body.name)?,
                description: body.description,
                kind: selected.kind.as_str().into(),
                definition: serde_json::to_value(selected).map_err(ApiError::internal)?,
                revision: 1,
                created_at: ctx.now,
                updated_at: ctx.now,
                deleted_at: None,
            };
            for member in &members {
                if let Some(child) = child(member)? {
                    rows::check_child(conn, &row, child).await?;
                }
            }
            let access = match body.access {
                NewResourceAccess::Defaults => NewAccess::Defaults,
                NewResourceAccess::OnlyMe => NewAccess::OnlyMe,
                NewResourceAccess::Home => NewAccess::Home,
            };
            Ok((
                Creation {
                    row,
                    members,
                    access,
                },
                sources,
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
            home(conn, principal, value.row.home_id).await?;
            Ok(())
        })
    }
}
impl effects::Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Creation,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<api::PoolReceipt>>> {
        Box::pin(async move {
            before.check()?;
            rows::insert(conn, &value.row).await?;
            ownership::start_resource(
                conn,
                ObjectRef {
                    kind: ObjectKind::Pool,
                    id: value.row.id,
                },
                principal.user(),
                value.access,
                Time::new(ctx.now),
            )
            .await
            .map_err(ApiError::internal)?;
            for member in &value.members {
                rows::put_member(conn, &value.row, member, child(member)?).await?;
            }
            record(ctx, conn, &value.row, "pool.created").await?;
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(principal.user()),
                "pool",
                value.row.id,
                serde_json::json!({"revision":value.row.revision}),
            )
            .await?;
            Ok(effects::Outcome::Applied(receipt(
                value.row.id,
                value.row.revision,
            )))
        })
    }
}
struct Update;
struct Change {
    row: Row,
    expected: i64,
}
impl effects::Effect for Update {
    type Source = api::Source;
    type Query = ();
    type Body = api::PoolUpdateBody;
    type Value = Change;
    type Output = api::PoolReceipt;
    fn declaration(_value: &Change) -> &'static Declaration<api::Source> {
        &api::UPDATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> native::SourceFuture<'a, Change, api::Source> {
        Box::pin(async move {
            let (_principal, mut row) = pin(ctx, conn, caller, &api::UPDATE).await?;
            if row.revision != body.expected_revision {
                return Err(stale());
            }
            let mut selected: Definition =
                serde_json::from_value(row.definition.clone()).map_err(ApiError::internal)?;
            selected.enabled = body.enabled;
            selected.policy = body.policy;
            selected.lend_members = body.lend_members;
            if let Some(config) = body.config {
                selected.config = config;
                definition(&selected)?;
                config_unlocked(conn, row.home_id, &selected.config).await?;
            } else {
                selected
                    .validate()
                    .map_err(|why| invalid("definition", &why))?;
            }
            row.name = name(&body.name)?;
            row.description = body.description;
            row.definition = serde_json::to_value(selected).map_err(ApiError::internal)?;
            row.updated_at = ctx.now;
            let sources = facts(conn, row.id).await?;
            Ok((
                Change {
                    row,
                    expected: body.expected_revision,
                },
                sources,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Change,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Pool,
            value.row.id,
            Permission::PoolManage,
        ))
    }
}
impl effects::Apply for Update {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Change,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<api::PoolReceipt>>> {
        Box::pin(async move {
            before.check()?;
            if !rows::update(conn, &value.row, value.expected).await? {
                return Err(stale());
            }
            orch_db::queries::access::bump_epoch_in_root(conn, value.row.org_id).await?;
            record(ctx, conn, &value.row, "pool.updated").await?;
            Ok(effects::Outcome::Applied(receipt(
                value.row.id,
                value.expected + 1,
            )))
        })
    }
}
struct PutMember;
struct MemberChange {
    change: Change,
    member: rows::Member,
}
impl effects::Effect for PutMember {
    type Source = api::Source;
    type Query = ();
    type Body = api::PoolMemberPutBody;
    type Value = MemberChange;
    type Output = api::PoolReceipt;
    fn declaration(_value: &MemberChange) -> &'static Declaration<api::Source> {
        &api::MEMBER_PUT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> native::SourceFuture<'a, MemberChange, api::Source> {
        Box::pin(async move {
            let (_principal, mut row) = pin(ctx, conn, caller, &api::MEMBER_PUT).await?;
            if row.revision != body.expected_revision {
                return Err(stale());
            }
            if ctx.id("member_id")? != body.member.id {
                return Err(invalid(
                    "member.id",
                    "the member identity must match its path",
                ));
            }
            let mut selected: Definition =
                serde_json::from_value(row.definition.clone()).map_err(ApiError::internal)?;
            selected.members = vec![body.member.clone()];
            selected.validate().map_err(|why| invalid("member", &why))?;
            let member = member(body.member, body.position)?;
            if let Some(child) = child(&member)? {
                rows::check_child(conn, &row, child).await?;
            }
            row.updated_at = ctx.now;
            let sources = facts(conn, row.id).await?;
            Ok((
                MemberChange {
                    change: Change {
                        row,
                        expected: body.expected_revision,
                    },
                    member,
                },
                sources,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a MemberChange,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Pool,
            value.change.row.id,
            Permission::PoolManage,
        ))
    }
}
impl effects::Apply for PutMember {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a MemberChange,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<api::PoolReceipt>>> {
        Box::pin(async move {
            before.check()?;
            if !rows::update(conn, &value.change.row, value.change.expected).await? {
                return Err(stale());
            }
            rows::put_member(
                conn,
                &value.change.row,
                &value.member,
                child(&value.member)?,
            )
            .await?;
            orch_db::queries::access::bump_epoch_in_root(conn, value.change.row.org_id).await?;
            record(ctx, conn, &value.change.row, "pool.member_updated").await?;
            Ok(effects::Outcome::Applied(receipt(
                value.change.row.id,
                value.change.expected + 1,
            )))
        })
    }
}
async fn finish<F: effects::Apply<Output = api::PoolReceipt>>(
    ctx: Ctx,
) -> ApiResult<api::PoolReceipt> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<F>(&ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&completed.ctx, &completed.output)?;
    Ok(completed.output)
}
async fn create(ctx: Ctx, (): (), _body: api::PoolCreateBody) -> ApiResult<api::PoolReceipt> {
    finish::<Create>(ctx).await
}
async fn update(ctx: Ctx, (): (), _body: api::PoolUpdateBody) -> ApiResult<api::PoolReceipt> {
    finish::<Update>(ctx).await
}
async fn put_member(
    ctx: Ctx,
    (): (),
    _body: api::PoolMemberPutBody,
) -> ApiResult<api::PoolReceipt> {
    finish::<PutMember>(ctx).await
}

struct Members;
#[derive(Clone, PartialEq, Eq)]
struct MemberPage {
    pool: Id,
    revision: i64,
    rows: Vec<rows::Member>,
    visible: Vec<bool>,
    next: Option<api::PoolMembersQuery>,
}
impl native::Selected for Members {
    type Source = api::Source;
    type Query = api::PoolMembersQuery;
    type Value = MemberPage;
    type Reply = api::PoolMembersReply;
    const DECLARATION: &'static Declaration<api::Source> = &api::MEMBERS;
    const WATCH_ORDINARY: bool = true;
    const MOVES: native::Moves = native::Moves::Repeat;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> native::SourceFuture<'a, MemberPage, api::Source> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let sources = facts(conn, id).await?;
            let known = native::bind_known(ctx, &api::MEMBERS, &sources)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(
                conn,
                &principal,
                ResourceKind::Pool,
                id,
                Permission::PoolView,
            )
            .await?;
            let row = rows::get(conn, id).await?.ok_or_else(ApiError::not_found)?;
            let after = match (query.after_position, query.after_id) {
                (None, None) => None,
                (Some(position), Some(id)) => Some((position, id)),
                _ => {
                    return Err(invalid(
                        "after_id",
                        "both member cursor fields are required together",
                    ));
                }
            };
            let batch = orch_settings::get::<i64>(
                conn,
                orch_settings::Place::Pool(id),
                orch_settings_contract::registry::keys::POOLS_READ_BATCH_SIZE,
            )
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the pool read batch has no registered default"))?;
            let mut selected = rows::members(
                conn,
                id,
                after,
                batch.checked_add(1).ok_or_else(|| {
                    invalid(
                        "pools.read_batch_size",
                        "the batch exceeds its stored integer representation",
                    )
                })?,
            )
            .await?;
            let has_more = i64::try_from(selected.len()).map_err(ApiError::internal)? > batch;
            selected.truncate(usize::try_from(batch).map_err(ApiError::internal)?);
            let next = has_more
                .then(|| {
                    selected.last().map(|member| api::PoolMembersQuery {
                        after_position: Some(member.position),
                        after_id: Some(member.id),
                    })
                })
                .flatten();
            let mut visible = Vec::with_capacity(selected.len());
            for member in &selected {
                let rule: MemberRule =
                    serde_json::from_value(member.rule.clone()).map_err(ApiError::internal)?;
                visible.push(rule_readable(conn, &principal, &rule).await?);
            }
            Ok((
                MemberPage {
                    pool: id,
                    revision: row.revision,
                    rows: selected,
                    visible,
                    next,
                },
                sources,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                ResourceKind::Pool,
                ctx.id("id")?,
                Permission::PoolView,
            )
            .await
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a MemberPage,
    ) -> BoxFuture<'a, ApiResult<api::PoolMembersReply>> {
        Box::pin(async move {
            let mut items = Vec::with_capacity(value.rows.len());
            for (row, readable) in value.rows.iter().zip(value.visible.iter().copied()) {
                let rule: MemberRule =
                    serde_json::from_value(row.rule.clone()).map_err(ApiError::internal)?;
                items.push(api::PoolMemberView {
                    id: row.id,
                    position: row.position,
                    enabled: row.enabled,
                    priority: row
                        .priority
                        .map(u64::try_from)
                        .transpose()
                        .map_err(ApiError::internal)?,
                    weight: row.weight.clone(),
                    rule: readable.then_some(rule),
                    reason: (!readable).then(|| "member_hidden".into()),
                });
            }
            Ok(api::PoolMembersReply {
                items,
                next: value.next.clone(),
            })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a MemberPage,
        fresh: &'a MemberPage,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}
async fn rule_readable(
    conn: &mut Conn,
    principal: &Principal,
    rule: &MemberRule,
) -> ApiResult<bool> {
    use orch_provider_contract::pools::Leaf;
    let mut targets = Vec::new();
    match rule {
        MemberRule::Pool { id, .. } => targets.push((ObjectKind::Pool, *id, Permission::PoolView)),
        MemberRule::ProviderModels { provider_id, .. } => {
            targets.push((ObjectKind::Provider, *provider_id, Permission::ProviderView))
        }
        MemberRule::HostEnvironments { host_id } => {
            targets.push((ObjectKind::Host, *host_id, Permission::HostView))
        }
        MemberRule::TemplateOnHost {
            template_id,
            host_id,
        } => {
            targets.push((ObjectKind::Template, *template_id, Permission::TemplateView));
            targets.push((ObjectKind::Host, *host_id, Permission::HostView));
        }
        MemberRule::Single { resource } => match resource {
            Leaf::Model { backend } => {
                let Some(offering) = backend.offering.as_ref() else {
                    return Ok(false);
                };
                targets.push((
                    ObjectKind::Provider,
                    offering.provider,
                    Permission::ProviderView,
                ));
            }
            Leaf::Provider { id } => {
                targets.push((ObjectKind::Provider, *id, Permission::ProviderView))
            }
            Leaf::Environment { id } => {
                targets.push((ObjectKind::Environment, *id, Permission::EnvironmentView))
            }
            Leaf::Host { id } => targets.push((ObjectKind::Host, *id, Permission::HostView)),
            Leaf::EnvironmentTemplate {
                template_id,
                host_id,
            } => {
                targets.push((ObjectKind::Template, *template_id, Permission::TemplateView));
                targets.push((ObjectKind::Host, *host_id, Permission::HostView));
            }
        },
        MemberRule::Matching { .. } => return Ok(true),
    }
    for (kind, id, permission) in targets {
        let decision = resources::check(
            conn,
            principal,
            ObjectRef { kind, id },
            permission,
            Policy::default(),
        )
        .await;
        match decision {
            Ok(decision) if decision.is_allowed() => {}
            Ok(_) | Err(orch_authz::Error::NotFound(_)) => return Ok(false),
            Err(error) => return Err(ApiError::internal(error)),
        }
    }
    Ok(true)
}

mod lending;
