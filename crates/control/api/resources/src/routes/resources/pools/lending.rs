//! Member-side consents use the original guarded sharing act, never pool-manager authority.
use super::*;
use orch_access_contract::{
    permissions::Profile,
    pools::{LendingMatcher, LendingScope},
    roles::PlaceRef,
};
use orch_authz::pages::{ShareJudgement, Target};

#[derive(sqlx::FromRow)]
struct ExistingConsent {
    resource_kind: String,
    resource_id: Id,
    is_defaults: bool,
    giver_id: Id,
    revision: i64,
    matcher: Json,
    options: Json,
    show_names: bool,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    enabled: bool,
}

#[derive(sqlx::FromRow)]
struct ListedConsent {
    id: Id,
    revision: i64,
    matcher: Json,
    options: Json,
    show_names: bool,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    giver_id: Id,
}

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_operation(api::PoolLendingPut, effects::prepare::<Set>, set);
    registry.on_snapshot_body_read_with_optional_operation(
        api::PoolLendingList,
        native::selected_body::<List, (), api::PoolLendingListBody>,
        native::selected_body_read::<List, (), api::PoolLendingListBody>,
    );
}
struct Set;
struct Change {
    body: api::PoolLendingPutBody,
    id: Id,
    expected: Option<i64>,
    kind: ObjectKind,
    source: Id,
    defaults: bool,
    revision: i64,
    replay: bool,
    giver: Id,
    declaration: &'static Declaration<api::Source>,
}
fn kind(scope: &LendingScope) -> Option<ObjectKind> {
    match scope {
        LendingScope::Member { object } => Some(object.kind),
        LendingScope::Defaults { resource_kind, .. } => Some((*resource_kind).into()),
    }
}
fn use_permission(kind: ObjectKind) -> Option<Permission> {
    Some(match kind {
        ObjectKind::Provider => Permission::ProviderUse,
        ObjectKind::Environment => Permission::EnvironmentUse,
        ObjectKind::Host => Permission::HostUse,
        ObjectKind::Pool => Permission::PoolUse,
        ObjectKind::Template => Permission::TemplateUse,
        _ => return None,
    })
}
async fn binding(
    conn: &mut Conn,
    scope: &LendingScope,
) -> ApiResult<(
    &'static Declaration<api::Source>,
    Vec<native::Facts<api::Source>>,
    ObjectKind,
    Id,
    bool,
)> {
    let kind = kind(scope)
        .filter(|kind| use_permission(*kind).is_some())
        .ok_or_else(|| invalid("scope", "this resource has no pool lending policy"))?;
    match scope {
        LendingScope::Member { object } => Ok((
            &api::LENDING_MEMBER,
            vec![native::Facts::one(
                api::Source::Subject,
                KeyTargetKind::Object(kind),
                crate::key_operations::targets::page_object(conn, *object).await?,
            )],
            kind,
            object.id,
            false,
        )),
        LendingScope::Defaults {
            at: PlaceRef::Node(id),
            ..
        } => {
            let (_, node_kind) = placed(conn, *id).await?;
            if !matches!(
                node_kind,
                orch_api_contract::nodes::NodeKind::Org
                    | orch_api_contract::nodes::NodeKind::Project
            ) {
                return Err(invalid(
                    "scope.at",
                    "pool lending defaults belong to an organization or project",
                ));
            }
            Ok((
                &api::LENDING_DEFAULTS,
                vec![native::Facts::one(
                    api::Source::Home,
                    KeyTargetKind::Object(
                        if node_kind == orch_api_contract::nodes::NodeKind::Org {
                            ObjectKind::Org
                        } else {
                            ObjectKind::Project
                        },
                    ),
                    crate::key_operations::targets::node(conn, *id).await?,
                )],
                kind,
                *id,
                true,
            ))
        }
        LendingScope::Defaults {
            at: PlaceRef::System,
            ..
        } => Ok((
            &api::LENDING_DEFAULTS,
            vec![native::Facts::one(
                api::Source::Home,
                KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::System),
                crate::key_operations::targets::system(),
            )],
            kind,
            Id::nil(),
            true,
        )),
    }
}
async fn policy(conn: &mut Conn, scope: &LendingScope) -> ApiResult<Policy> {
    let home = match scope {
        LendingScope::Member { object } => {
            resources::site(conn, *object)
                .await
                .map_err(ApiError::internal)?
                .1
                .home
        }
        LendingScope::Defaults {
            at: PlaceRef::Node(id),
            ..
        } => Some(*id),
        LendingScope::Defaults {
            at: PlaceRef::System,
            ..
        } => None,
    };
    let at = home.map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let holders_share = orch_settings::get::<bool>(conn, at, "sharing.allow_holders")
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("the sharing policy has no registered default"))?;
    let owners_name_outsiders = orch_settings::get::<bool>(conn, at, "sharing.allow_external")
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::internal("the outside-sharing policy has no registered default")
        })?;
    Ok(Policy {
        holders_share,
        owners_name_outsiders,
    })
}
async fn require_share(
    conn: &mut Conn,
    principal: &Principal,
    scope: &LendingScope,
    options: &Json,
) -> ApiResult<()> {
    let kind = kind(scope).ok_or_else(|| invalid("scope", "choose a member"))?;
    let permission = use_permission(kind)
        .ok_or_else(|| invalid("scope", "this resource has no lending policy"))?;
    let parsed = orch_access_contract::permissions::Options::parse(permission, options).map_err(
        |faults| {
            invalid("options", "the lending limits are invalid")
                .with("errors", field_faults(faults))
        },
    )?;
    let target = match scope {
        LendingScope::Member { object } => Target::Resource(*object),
        LendingScope::Defaults { at, .. } => Target::Defaults(*at),
    };
    let profile = match scope {
        LendingScope::Member { .. } => Profile::Any,
        LendingScope::Defaults { .. } => {
            permission.profile(orch_access_contract::permissions::Placement::Managed)
        }
    };
    let sharing_policy = policy(conn, scope).await?;
    match orch_authz::pages::judge_share(
        conn,
        principal,
        target,
        &[(permission, profile)],
        sharing_policy,
    )
    .await
    .map_err(ApiError::internal)?
    {
        ShareJudgement::Allowed(_) => {}
        ShareJudgement::Refused(refusal) => {
            return Err(crate::middleware::authorization::refusals::denied(conn, *refusal).await?);
        }
    }
    if let LendingScope::Member { object } = scope {
        let policies = orch_engine::access::resource_policies(conn, object.id)
            .await
            .map_err(ApiError::internal)?;
        if object.kind == ObjectKind::Pool {
            let valid = match &parsed {
                orch_access_contract::permissions::Options::PoolUse(requested) => {
                    orch_authz::pools::pool_allowances(conn, principal, object.id, None, &policies)
                        .await
                        .map_err(ApiError::internal)?
                        .iter()
                        .any(|(_, own)| requested.within(own))
                }
                _ => false,
            };
            if !valid {
                return Err(invalid(
                    "options",
                    "a lending entry cannot exceed one complete current pool allowance",
                ));
            }
        }
        if object.kind == ObjectKind::Provider {
            let usage = orch_authz::usable::usable(conn, principal, object.id, None, &policies)
                .await
                .map_err(ApiError::internal)?;
            let valid = match (&parsed, usage.granted()) {
                (
                    orch_access_contract::permissions::Options::ProviderUse(requested),
                    Some(usage),
                ) => usage
                    .each
                    .iter()
                    .any(|entry| requested.within(&entry.provider)),
                _ => false,
            };
            if !valid {
                return Err(invalid(
                    "options",
                    "a lending entry cannot exceed one complete current provider allowance",
                ));
            }
        }
        if matches!(object.kind, ObjectKind::Host | ObjectKind::Template) {
            let usage = orch_authz::usable::usable(conn, principal, object.id, None, &policies)
                .await
                .map_err(ApiError::internal)?;
            let mut requested = orch_access_contract::pools::PoolUse::unlimited();
            let mut own = orch_access_contract::pools::PoolUse::unlimited();
            let valid = match (&parsed, usage.granted().map(|usage| &usage.options)) {
                (
                    orch_access_contract::permissions::Options::HostUse(value),
                    Some(orch_access_contract::permissions::Options::HostUse(held)),
                ) => {
                    requested.host = value.clone();
                    own.host = held.clone();
                    true
                }
                (
                    orch_access_contract::permissions::Options::TemplateUse(value),
                    Some(orch_access_contract::permissions::Options::TemplateUse(held)),
                ) => {
                    requested.template = value.clone();
                    own.template = held.clone();
                    true
                }
                _ => false,
            };
            if !valid || !requested.within(&own) {
                return Err(invalid(
                    "options",
                    "a lending entry cannot exceed the giver's current limits",
                ));
            }
        }
    }
    Ok(())
}
impl effects::Effect for Set {
    type Source = api::Source;
    type Query = ();
    type Body = api::PoolLendingPutBody;
    type Value = Change;
    type Output = api::PoolReceipt;
    fn declaration(value: &Change) -> &'static Declaration<api::Source> {
        value.declaration
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> native::SourceFuture<'a, Change, api::Source> {
        Box::pin(async move {
            match &body.matcher {
                LendingMatcher::Pool { pool_id } if pool_id.is_nil() => {
                    return Err(invalid("matcher.pool_id", "choose a pool"));
                }
                LendingMatcher::Home { home_id } if home_id.is_nil() => {
                    return Err(invalid("matcher.home_id", "choose a home"));
                }
                LendingMatcher::Managers { manager_ids }
                    if manager_ids.is_empty() || manager_ids.iter().any(|id| id.is_nil()) =>
                {
                    return Err(invalid(
                        "matcher.manager_ids",
                        "choose at least one manager",
                    ));
                }
                _ => {}
            }
            if body.enabled && body.expires_at.is_some_and(|end| end.get() <= ctx.now) {
                return Err(invalid("expires_at", "choose a future expiry"));
            }
            let (declaration, sources, kind, source, defaults) = binding(conn, &body.scope).await?;
            let known = native::bind_known(ctx, declaration, &sources)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            if body.enabled {
                require_share(conn, &principal, &body.scope, &body.options).await?;
            }
            match &body.scope {
                LendingScope::Member { object } => {
                    let resource_kind = object
                        .kind
                        .resource()
                        .ok_or_else(|| invalid("scope", "choose a resource"))?;
                    if !orch_db::queries::resources::grouping::lock(conn, resource_kind, object.id)
                        .await?
                    {
                        return Err(ApiError::not_found());
                    }
                }
                LendingScope::Defaults {
                    at: PlaceRef::Node(id),
                    ..
                } => super::super::creation_home::lock(conn, Some(*id)).await?,
                LendingScope::Defaults {
                    at: PlaceRef::System,
                    ..
                } => {}
            }
            let id = ctx.id("entry_id")?;
            let existing:Option<ExistingConsent>=sqlx::query_as("SELECT resource_kind,resource_id,is_defaults,giver_id,revision,matcher,options,show_names,expires_at,removed_at IS NULL AS enabled FROM pool_lending WHERE id=$1 FOR UPDATE")
                .bind(id).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
            let mut replay = false;
            let mut entry_giver = principal.user();
            let revision = match existing {
                Some(ExistingConsent {
                    resource_kind: stored_kind,
                    resource_id: stored_id,
                    is_defaults: stored_defaults,
                    giver_id: giver,
                    revision,
                    matcher,
                    options,
                    show_names,
                    expires_at,
                    enabled,
                }) => {
                    if stored_kind != kind.as_str()
                        || stored_id != source
                        || stored_defaults != defaults
                    {
                        return Err(stale());
                    }
                    if body.expected_revision.is_none()
                        && revision == 1
                        && giver == principal.user()
                        && matcher
                            == serde_json::to_value(&body.matcher).map_err(ApiError::internal)?
                        && options == body.options
                        && show_names == body.show_names
                        && expires_at == body.expires_at.map(|end| end.get())
                        && enabled == body.enabled
                    {
                        replay = true;
                    } else if body.expected_revision != Some(revision) {
                        return Err(stale());
                    }
                    entry_giver = giver;
                    if !body.enabled {
                        require_revoke(conn, &principal, &body.scope, giver).await?;
                    } else if giver != principal.user() {
                        return Err(invalid(
                            "entry_id",
                            "another giver's consent may only be withdrawn",
                        ));
                    }
                    if replay {
                        revision
                    } else {
                        revision.checked_add(1).ok_or_else(|| {
                            ApiError::internal("the consent revision is exhausted")
                        })?
                    }
                }
                None => {
                    if body.expected_revision.is_some() || !body.enabled {
                        return Err(stale());
                    }
                    1
                }
            };
            let expected = body.expected_revision;
            Ok((
                Change {
                    body,
                    id,
                    expected,
                    kind,
                    source,
                    defaults,
                    revision,
                    replay,
                    giver: entry_giver,
                    declaration,
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
        Box::pin(async move {
            if value.body.enabled {
                require_share(conn, principal, &value.body.scope, &value.body.options).await
            } else {
                require_revoke(conn, principal, &value.body.scope, value.giver).await
            }
        })
    }
}
impl effects::Apply for Set {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Change,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<api::PoolReceipt>>> {
        Box::pin(async move {
            before.check()?;
            if value.replay {
                return Ok(effects::Outcome::Applied(receipt(value.id, value.revision)));
            }
            let changed = if value.expected.is_none() {
                sqlx::query("INSERT INTO pool_lending(id,resource_kind,resource_id,is_defaults,giver_id,matcher,options,show_names,expires_at,removed_at,created_at,updated_at)
                    VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,CASE WHEN $10 THEN NULL ELSE $11 END,$11,$11)")
                    .bind(value.id).bind(value.kind.as_str()).bind(value.source).bind(value.defaults).bind(principal.user())
                    .bind(serde_json::to_value(&value.body.matcher).map_err(ApiError::internal)?).bind(&value.body.options).bind(value.body.show_names)
                    .bind(value.body.expires_at.map(|end|end.get())).bind(value.body.enabled).bind(ctx.now).execute(&mut *conn).await.map_err(DbError::from)?.rows_affected()
            } else if !value.body.enabled {
                sqlx::query("UPDATE pool_lending SET removed_at=$3,updated_at=$3,revision=revision+1 WHERE id=$1 AND revision=$2")
                    .bind(value.id).bind(value.expected).bind(ctx.now).execute(&mut *conn).await.map_err(DbError::from)?.rows_affected()
            } else {
                sqlx::query("UPDATE pool_lending SET matcher=$3,options=$4,show_names=$5,renewed_at=CASE WHEN expires_at IS DISTINCT FROM $6 THEN $8 ELSE renewed_at END,
                    expires_at=$6,removed_at=CASE WHEN $7 THEN NULL ELSE $8 END,updated_at=$8,revision=revision+1 WHERE id=$1 AND revision=$2 AND giver_id=$9")
                    .bind(value.id).bind(value.expected).bind(serde_json::to_value(&value.body.matcher).map_err(ApiError::internal)?).bind(&value.body.options).bind(value.body.show_names)
                    .bind(value.body.expires_at.map(|end|end.get())).bind(value.body.enabled).bind(ctx.now).bind(principal.user()).execute(&mut *conn).await.map_err(DbError::from)?.rows_affected()
            };
            if changed != 1 {
                return Err(stale());
            }
            let root = match &value.body.scope {
                LendingScope::Member { object } => resources::site(conn, *object)
                    .await
                    .map_err(ApiError::internal)?
                    .1
                    .root
                    .unwrap_or(orch_db::queries::transaction_guard::SYSTEM),
                LendingScope::Defaults {
                    at: PlaceRef::Node(id),
                    ..
                } => placed(conn, *id).await?.0.org,
                LendingScope::Defaults {
                    at: PlaceRef::System,
                    ..
                } => orch_db::queries::transaction_guard::SYSTEM,
            };
            orch_db::queries::access::bump_epoch_in_root(conn, root).await?;
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(principal.user()),
                orch_access_contract::audit::AuditAction::ResourcePoolLendingChanged.as_str(),
                value.kind.as_str(),
                Some(value.source),
                "Changed lending through pools",
            );
            entry.details = serde_json::json!({"entry_id":value.id,"revision":value.revision,"enabled":value.body.enabled});
            entry.history = true;
            orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
            Ok(effects::Outcome::Applied(receipt(value.id, value.revision)))
        })
    }
}
async fn set(ctx: Ctx, (): (), _body: api::PoolLendingPutBody) -> ApiResult<api::PoolReceipt> {
    finish::<Set>(ctx).await
}

async fn management(
    conn: &mut Conn,
    principal: &Principal,
    scope: &LendingScope,
) -> ApiResult<bool> {
    let target = match scope {
        LendingScope::Member { object } => Target::Resource(*object),
        LendingScope::Defaults { at, .. } => Target::Defaults(*at),
    };
    let sharing_policy = policy(conn, scope).await?;
    let page = orch_authz::pages::read(conn, principal, target, sharing_policy)
        .await
        .map_err(ApiError::internal)?;
    let permission =
        use_permission(kind(scope).ok_or_else(|| invalid("scope", "choose a member"))?)
            .ok_or_else(|| invalid("scope", "choose a lendable resource"))?;
    Ok(page.writable
        && page
            .rows
            .iter()
            .any(|row| row.permission == permission && row.writable))
}
async fn require_revoke(
    conn: &mut Conn,
    principal: &Principal,
    scope: &LendingScope,
    giver: Id,
) -> ApiResult<()> {
    if giver == principal.user() || management(conn, principal, scope).await? {
        Ok(())
    } else {
        Err(invalid(
            "entry_id",
            "withdrawing this consent needs its giver or the member's permission manager",
        ))
    }
}
struct List;
struct Listed {
    declaration: &'static Declaration<api::Source>,
    items: Vec<api::PoolLendingView>,
    next: Option<Id>,
}
impl native::Selected for List {
    type Source = api::Source;
    type Query = ((), api::PoolLendingListBody);
    type Value = Listed;
    type Reply = api::PoolLendingReply;
    const DECLARATION: &'static Declaration<api::Source> = &api::LENDING_LIST_MEMBER;
    const WATCH_ORDINARY: bool = true;
    const MOVES: native::Moves = native::Moves::Repeat;
    fn declaration(_: &Self::Query, value: &Listed) -> &'static Declaration<api::Source> {
        value.declaration
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> native::SourceFuture<'a, Listed, api::Source> {
        Box::pin(async move {
            let body = &query.1;
            let (_, sources, kind, source, defaults) = binding(conn, &body.scope).await?;
            let declaration = if defaults {
                &api::LENDING_LIST_DEFAULTS
            } else {
                &api::LENDING_LIST_MEMBER
            };
            let known = native::bind_known(ctx, declaration, &sources)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let manages = management(conn, &principal, &body.scope).await?;
            let at = match &body.scope {
                LendingScope::Member { object } if object.kind == ObjectKind::Pool => {
                    orch_settings::Place::Pool(object.id)
                }
                LendingScope::Member { object } => resources::site(conn, *object)
                    .await
                    .map_err(ApiError::internal)?
                    .1
                    .home
                    .map_or(orch_settings::Place::System, orch_settings::Place::Node),
                LendingScope::Defaults {
                    at: PlaceRef::Node(id),
                    ..
                } => orch_settings::Place::Node(*id),
                LendingScope::Defaults {
                    at: PlaceRef::System,
                    ..
                } => orch_settings::Place::System,
            };
            let batch = orch_settings::get::<i64>(
                conn,
                at,
                orch_settings_contract::registry::keys::POOLS_READ_BATCH_SIZE,
            )
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the pool read batch has no default"))?;
            let mut rows: Vec<ListedConsent>=sqlx::query_as("SELECT id,revision,matcher,options,show_names,expires_at,giver_id FROM pool_lending WHERE resource_kind=$1 AND resource_id=$2 AND is_defaults=$3 AND removed_at IS NULL AND ($4 OR giver_id=$5) AND ($6::uuid IS NULL OR id>$6) ORDER BY id LIMIT $7")
                .bind(kind.as_str()).bind(source).bind(defaults).bind(manages).bind(principal.user()).bind(body.after_id).bind(batch.checked_add(1).ok_or_else(||invalid("pools.read_batch_size","the batch exceeds its representation"))?).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            let more = i64::try_from(rows.len()).map_err(ApiError::internal)? > batch;
            rows.truncate(usize::try_from(batch).map_err(ApiError::internal)?);
            let next = if more {
                rows.last().map(|row| row.id)
            } else {
                None
            };
            let items = rows
                .into_iter()
                .map(
                    |ListedConsent {
                         id,
                         revision,
                         matcher,
                         options,
                         show_names,
                         expires_at,
                         giver_id: giver,
                     }| {
                        Ok(api::PoolLendingView {
                            id,
                            revision,
                            matcher: serde_json::from_value(matcher).map_err(ApiError::internal)?,
                            options,
                            show_names,
                            expires_at: expires_at.map(Time::new),
                            giver: None,
                            can_revoke: manages || giver == principal.user(),
                        })
                    },
                )
                .collect::<ApiResult<Vec<_>>>()?;
            Ok((
                Listed {
                    declaration,
                    items,
                    next,
                },
                sources,
            ))
        })
    }
    fn require<'a>(_: &'a Ctx, _: &'a mut Conn, _: &'a Principal) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Listed,
    ) -> BoxFuture<'a, ApiResult<api::PoolLendingReply>> {
        Box::pin(async move {
            Ok(api::PoolLendingReply {
                items: value.items.clone(),
                next: value.next,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        opened: &'a Listed,
        fresh: &'a Listed,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.items == fresh.items && opened.next == fresh.next) })
    }
}
