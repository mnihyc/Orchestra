//! Actual channel owner namespaces and the independent native own-route projection.

use super::*;
use crate::key_operations::{admission, native, targets};
use crate::registry::OperationPlan;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_api_contract::notification_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_authz::{Caller, engine::Graph};

pub(crate) struct Channels;
const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
pub(crate) struct Selected {
    intent: Intent,
    views: PreparedViews,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Intent {
    Available,
    Org(Id),
    System,
}
impl Intent {
    fn of(query: &NotificationChannelListQuery) -> ApiResult<Self> {
        match (query.org_id, query.system.unwrap_or(false)) {
            (Some(_), true) => Err(ApiError::invalid("system", "select one channel owner")),
            (Some(org), false) => Ok(Self::Org(org)),
            (None, true) => Ok(Self::System),
            (None, false) => Ok(Self::Available),
        }
    }
    fn declaration(self) -> &'static Declaration<Source> {
        match self {
            Self::Available => &sources::CHANNEL_LIST_AVAILABLE,
            Self::Org(_) | Self::System => &sources::CHANNEL_LIST_MANAGE,
        }
    }
    async fn require(self, ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
        let result: ApiResult<()> = async {
            match self {
                Self::Available => Ok(()),
                Self::Org(org) => {
                    authorization::require(
                        conn,
                        principal,
                        org,
                        exact(Permission::SettingsManage),
                        Act::Use,
                        Policy::default(),
                    )
                    .await?;
                    root_org(conn, org, ctx).await
                }
                Self::System if system_channel_admin(principal) => Ok(()),
                Self::System => Err(refusals::system_admin_only(
                    "only a system admin looks after the system's channels",
                )),
            }
        }
        .await;
        result.map_err(|error| {
            if native::modern(principal.caller()) {
                authorization::problem_without_content(error)
            } else {
                error
            }
        })
    }
    async fn rows(self, conn: &mut Conn, user: Id) -> ApiResult<Vec<ChannelRow>> {
        Ok(match self {
            Self::Available => pages::channels_reaching(conn, user).await?,
            Self::Org(org) => pages::channels_of_org(conn, org).await?,
            Self::System => pages::system_channels(conn).await?,
        })
    }
    async fn management(self, conn: &mut Conn) -> ApiResult<Vec<native::Facts<Source>>> {
        let (family, target) = match self {
            Self::Available => return Ok(Vec::new()),
            Self::Org(org) => (
                KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org),
                targets::node(conn, org).await?,
            ),
            Self::System => (
                KeyTargetKind::Identity(KeyIdentityKind::System),
                targets::system(),
            ),
        };
        if target.object.kind != family {
            return Err(ApiError::not_found());
        }
        Ok(vec![native::Facts::one(
            Source::ChannelOwner,
            family,
            target,
        )])
    }
}

type Owner = (Option<Id>, Option<Id>);
type Owners = BTreeMap<Owner, KeyResolvedTarget>;

fn owner_family(row: &ChannelRow) -> KeyTargetKind {
    match (row.org_id, row.user_id) {
        (Some(_), _) => KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org),
        (None, Some(_)) => ACCOUNT,
        (None, None) => KeyTargetKind::Identity(KeyIdentityKind::System),
    }
}

async fn owners(ctx: &Ctx, conn: &mut Conn, rows: &[ChannelRow]) -> ApiResult<Owners> {
    let orgs = rows
        .iter()
        .filter_map(|row| row.org_id)
        .collect::<BTreeSet<_>>();
    let graph = Graph::load(conn, &orgs.into_iter().collect::<Vec<_>>())
        .await
        .map_err(authorization::restoration_error)?;
    let account = if rows
        .iter()
        .any(|row| row.org_id.is_none() && row.user_id.is_some())
    {
        Some(targets::from_account(conn, &ctx.user()?.user).await?)
    } else {
        None
    };
    let mut found = BTreeMap::new();
    for row in rows {
        let target = match (row.org_id, row.user_id) {
            (Some(org), _) => orch_engine::operation_scope::targets::from_node_identity(
                &graph, org,
            )
            .filter(|target| {
                target.object.kind
                    == KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org)
            }),
            (None, Some(person)) if person == ctx.user()?.id() => account.clone(),
            (None, Some(_)) => None,
            (None, None) => Some(targets::system()),
        };
        if let Some(target) = target {
            found.insert((row.org_id, row.user_id), target);
        }
    }
    Ok(found)
}

fn selected_owners(rows: &[ChannelRow], owners: &Owners) -> native::Facts<Source> {
    let mut selected = BTreeMap::new();
    for row in rows {
        if let Some(target) = owners.get(&(row.org_id, row.user_id)) {
            selected.insert((owner_family(row), target.object), target.clone());
        }
    }
    let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
    for ((kind, _), target) in selected {
        groups.entry(kind).or_default().push(target);
    }
    native::Facts::collection(Source::AvailableChannelOwners, groups.into_iter().collect())
}

async fn mail_filter(
    conn: &mut Conn,
    principal: &Principal,
    rows: &mut Vec<ChannelRow>,
) -> ApiResult<()> {
    if !system_channel_admin(principal)
        && !channels::mail_may_be_shared(conn).await.map_err(internal)?
    {
        rows.retain(|row| !(row.is_the_systems() && row.kind == "email"));
    }
    Ok(())
}

impl native::Selected for Channels {
    type Source = Source;
    type Query = NotificationChannelListQuery;
    type Value = Selected;
    type Reply = Vec<NotificationChannel>;
    const DECLARATION: &'static Declaration<Source> = &sources::CHANNEL_LIST_AVAILABLE;
    fn declaration(_query: &Self::Query, value: &Selected) -> &'static Declaration<Source> {
        value.intent.declaration()
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let intent = Intent::of(query)?;
            let row = intent.declaration();
            let empty = OperationPlan::new(ctx.route, row.operation)?;
            if !native::modern(caller) {
                let principal = native::preparation_principal(ctx, conn, caller, &empty).await?;
                intent.require(ctx, conn, &principal).await?;
                let mut rows = intent.rows(conn, caller.user()).await?;
                if intent == Intent::Available {
                    mail_filter(conn, &principal, &mut rows).await?;
                }
                let views = prepare_views(conn, &principal, &rows, true).await?;
                return Ok((Selected { intent, views }, Vec::new()));
            }
            empty
                .shared()
                .compatible(caller)
                .map_err(admission::binding_error)?;
            let mut facts = intent.management(conn).await?;
            if intent != Intent::Available {
                let known = native::bind_known(ctx, row, &facts)?;
                let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
                intent.require(ctx, conn, &principal).await?;
            }
            let mut rows = intent.rows(conn, caller.user()).await?;
            if intent == Intent::Available {
                let owners = owners(ctx, conn, &rows).await?;
                let mut selected = Vec::with_capacity(rows.len());
                for channel in rows {
                    let Some(target) = owners.get(&(channel.org_id, channel.user_id)) else {
                        continue;
                    };
                    if native::permits(
                        &empty,
                        row,
                        caller,
                        &Source::AvailableChannelOwners,
                        owner_family(&channel),
                        target,
                    )? {
                        selected.push(channel);
                    }
                }
                rows = selected;
                let preliminary = native::bind_known(ctx, row, &[selected_owners(&rows, &owners)])?;
                let principal =
                    native::preparation_principal(ctx, conn, caller, &preliminary).await?;
                mail_filter(conn, &principal, &mut rows).await?;
                facts.push(selected_owners(&rows, &owners));
            }
            let account = if rows.is_empty() {
                None
            } else {
                Some(targets::from_account(conn, &ctx.user()?.user).await?)
            };
            let private_routes = match account.as_ref() {
                Some(account) => native::permits_usage(
                    &empty,
                    row,
                    caller,
                    &Source::ChannelCallerRoutes,
                    FacetUsage::ResponseContent,
                    ACCOUNT,
                    account,
                )?,
                None => false,
            };
            facts.push(native::Facts::collection(
                Source::ChannelCallerRoutes,
                if private_routes {
                    account
                        .map(|account| vec![(ACCOUNT, vec![account])])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
            ));
            let plan = native::bind(ctx, row, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let projection = ctx.clone();
            let views = plan
                .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                    Box::pin(async move {
                        intent.require(&projection, conn, reader).await?;
                        prepare_views(conn, reader, &rows, private_routes).await
                    })
                })
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the native channel projection is closed",
                    )
                })?;
            Ok((Selected { intent, views }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            value.intent.require(ctx, conn, principal).await?;
            Ok(value.views.render())
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.intent == fresh.intent && fresh.views.preserves(&opened.views))
        })
    }
}
