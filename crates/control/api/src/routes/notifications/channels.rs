//! Channels, the ways out of the application: listed for the caller with their own route, set up by the system, a root org or a person, changed, removed and tried.
//!
//! A channel is made from a notifier: its fields are checked against what the
//! notifier asks for, the secret ones are sealed under the channel and the
//! name of the field and are never returned, its own texts are checked as they
//! are saved, and then the notifier looks at the far side. What the far side
//! refuses for good, a password or a token, refuses the save with what it
//! answered; a far side that cannot be reached now does not, and the channel
//! is kept with that as its last error, since a server that is down today is
//! not a wrong entry. The system's channels are not held to the list of
//! private hosts; those of a root org and of a person are.

use super::views::{self, ChannelFacts};
use super::{master_key, notifiers, surroundings, toolbox};
use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::sign_in;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::notifications::{
    MeNotificationChannelCreateBody, NOTIFICATION_NAME_MAX_CHARS, NotificationChannel,
    NotificationChannelCreateBody, NotificationChannelListQuery, NotificationChannelTestBody,
    NotificationChannelUpdateBody, NotificationTestReply, TemplatesByLanguage,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::hierarchy;
use orch_db::queries::notifications::channels::{
    self as rows, ChannelChange, ChannelRow, NewChannel, owner,
};
use orch_db::queries::notifications::pages;
use orch_db::queries::notifications::people;
use orch_db::queries::notifications::routes as route_rows;
use orch_db::queries::settings;
use orch_db::{Conn, DbError};
use orch_notifier_contract::delivery::{DeliverError, Saved};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Target};
use orch_notifier_contract::templates::{Form, TemplateOverride};
use orch_types::{Id, Json};
use orch_worker_core::channels::{
    self, Given, SaveFault, TestFor, check_before_save, config_of_channel, outbound_hosts,
    seal_secret,
};
use orch_worker_core::routes as route_logic;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Whose a channel being set up is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Maker {
    /// The system's, by a system admin.
    System,
    /// A root org's, by whoever holds `settings.manage` on it.
    Org(Id),
    /// A person's own.
    Person(Id),
}

impl Maker {
    fn owner(self) -> Owner {
        match self {
            Self::System => Owner::System,
            Self::Org(_) => Owner::Org,
            Self::Person(_) => Owner::User,
        }
    }
}

fn faults(list: Vec<FieldFault>) -> ApiError {
    Problem::invalid(list).into()
}

/// The word a form of a text goes by in a path.
const fn form_word(form: Form) -> &'static str {
    match form {
        Form::Single => "single",
        Form::Digest => "digest",
    }
}

/// What is wrong with what was given, as the faults of the request, each
/// naming its member: a field is under `config` or, when it is a secret one,
/// under `secrets`.
pub(super) fn save_fault(descriptor: Option<&NotifierDescriptor>, fault: SaveFault) -> ApiError {
    match fault {
        SaveFault::UnknownKind(kind) => {
            ApiError::invalid("kind", &format!("there is no notifier named {kind}"))
        }
        SaveFault::WrongTarget { kind } => {
            ApiError::invalid("kind", &format!("{kind} cannot be set up for this"))
        }
        SaveFault::Fields(list) => faults(
            list.into_iter()
                .map(|fault| {
                    let secret = descriptor
                        .and_then(|each| each.fields.iter().find(|field| field.name == fault.path))
                        .is_some_and(|field| field.kind.is_secret());
                    let member = if secret { "secrets" } else { "config" };
                    FieldFault {
                        path: if fault.path.is_empty() {
                            member.to_owned()
                        } else {
                            format!("{member}.{}", fault.path)
                        },
                        message: fault.message,
                    }
                })
                .collect(),
        ),
        SaveFault::Texts(list) => faults(
            list.into_iter()
                .map(|fault| FieldFault {
                    path: format!("templates.{}.{}", fault.language, form_word(fault.form)),
                    message: fault.error.to_string(),
                })
                .collect(),
        ),
    }
}

/// The refusal of a save that the far side refused for good.
pub(super) fn refused(error: &DeliverError) -> ApiError {
    ApiError::invalid(
        "config",
        &format!("the far side refuses it: {}", error.message),
    )
}

/// A name of a channel or a destination, as it is kept: one to the most
/// characters, without the blanks around it.
pub(super) fn checked_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    let length = name.chars().count();
    let most = usize::try_from(NOTIFICATION_NAME_MAX_CHARS).unwrap_or(usize::MAX);
    if length == 0 || length > most {
        return Err(ApiError::invalid(
            "name",
            &format!("is from 1 to {NOTIFICATION_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}

/// What a person gave for a notifier's fields, as one answer: the values of
/// `config`, and the secrets by their names. A secret field is given under
/// `secrets` and nowhere else, and only a secret field is.
pub(super) fn answer_of(
    descriptor: &NotifierDescriptor,
    config: &Json,
    secrets: Option<&BTreeMap<String, String>>,
) -> ApiResult<Json> {
    let Json::Object(given) = config else {
        return Err(ApiError::invalid(
            "config",
            "is an object of the notifier's fields",
        ));
    };
    let is_secret = |name: &str| {
        descriptor
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.kind.is_secret())
    };
    let mut answer = given.clone();
    for name in given.keys() {
        if is_secret(name) == Some(true) {
            return Err(ApiError::invalid(
                &format!("config.{name}"),
                "is a secret: give it under secrets",
            ));
        }
    }
    for (name, value) in secrets.into_iter().flatten() {
        if is_secret(name) != Some(true) {
            return Err(ApiError::invalid(
                &format!("secrets.{name}"),
                "is not a secret field of this notifier",
            ));
        }
        answer.insert(name.clone(), Json::String(value.clone()));
    }
    Ok(Json::Object(answer))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error)
}

/// The texts as given, checked and kept in the form the database holds.
fn templates_of(
    given: Option<&TemplatesByLanguage>,
) -> ApiResult<(BTreeMap<String, TemplateOverride>, Json)> {
    let Some(given) = given else {
        return Ok((BTreeMap::new(), json!({})));
    };
    let overrides = views::overrides_of(given).map_err(internal)?;
    let stored = serde_json::to_value(given).map_err(internal)?;
    Ok((overrides, stored))
}

/// Whether the caller may change the channel: a system admin for the system's,
/// whoever holds `settings.manage` on the root org for its, the person for
/// their own.
async fn may_change(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChannelRow,
    managed: &mut HashMap<Id, bool>,
) -> ApiResult<bool> {
    match (row.org_id, row.user_id) {
        (Some(org), _) => {
            if let Some(known) = managed.get(&org) {
                return Ok(*known);
            }
            let held = matches!(
                orch_authz::nodes::check_named(
                    conn,
                    principal,
                    org,
                    exact(Permission::SettingsManage),
                    Act::Use,
                    Policy::default()
                )
                .await,
                Ok(orch_access_contract::decisions::Decision::Allowed { .. })
            );
            managed.insert(org, held);
            Ok(held)
        }
        (None, Some(person)) => Ok(person == principal.user()),
        (None, None) => Ok(principal.is_system_admin()),
    }
}

/// Refuses the caller who may not change the channel, with what asks them to
/// look elsewhere: a stranger's own channel is not there for them.
async fn require_change(conn: &mut Conn, principal: &Principal, row: &ChannelRow) -> ApiResult<()> {
    match (row.org_id, row.user_id) {
        (Some(org), _) => {
            authorization::require(
                conn,
                principal,
                org,
                exact(Permission::SettingsManage),
                Act::Use,
                Policy::default(),
            )
            .await
        }
        (None, Some(person)) if person == principal.user() => Ok(()),
        (None, Some(_)) => Err(ApiError::not_found()),
        (None, None) if principal.is_system_admin() => Ok(()),
        (None, None) => Err(refusals::system_admin_only(
            "only a system admin changes a channel of the system",
        )),
    }
}

/// The names of the secrets that have a value, for each of these channels.
async fn secret_names(
    conn: &mut Conn,
    channels: &[&ChannelRow],
) -> ApiResult<HashMap<Id, Vec<String>>> {
    let owners: Vec<(&str, Id)> = channels
        .iter()
        .map(|row| (owner::CHANNEL, row.id))
        .collect();
    let mut names: HashMap<Id, Vec<String>> = HashMap::new();
    for secret in settings::secret_names_of(conn, &owners).await? {
        // A person's waiting link code is kept with the channel, and is nobody else's to see.
        if secret
            .name
            .starts_with(orch_worker_core::bot_links::LINK_SECRET_PREFIX)
        {
            continue;
        }
        names.entry(secret.owner_id).or_default().push(secret.name);
    }
    for list in names.values_mut() {
        list.sort();
    }
    Ok(names)
}

/// The channels as the caller reads them, each with their own route.
async fn views_for(
    conn: &mut Conn,
    principal: &Principal,
    channels: &[ChannelRow],
) -> ApiResult<Vec<NotificationChannel>> {
    let person = people::recipient(conn, principal.user())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut managed: HashMap<Id, bool> = HashMap::new();
    let mut editable: Vec<bool> = Vec::with_capacity(channels.len());
    for row in channels {
        editable.push(may_change(conn, principal, row, &mut managed).await?);
    }
    let those_edited: Vec<&ChannelRow> = channels
        .iter()
        .zip(&editable)
        .filter(|(_, can)| **can)
        .map(|(row, _)| row)
        .collect();
    let mut secrets = secret_names(conn, &those_edited).await?;
    let org_ids: Vec<Id> = channels
        .iter()
        .filter_map(|row| row.org_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let orgs: BTreeMap<Id, NodeRef> = people::places(conn, &org_ids)
        .await?
        .into_iter()
        .map(|place| {
            (
                place.id,
                NodeRef {
                    thread_display: None,
                    id: place.id,
                    kind: NodeKind::Org,
                    name: Some(place.name),
                    redacted: false,
                },
            )
        })
        .collect();
    let mut routes: HashMap<Id, route_rows::RouteRow> = pages::routes_of(conn, principal.user())
        .await?
        .into_iter()
        .filter_map(|route| route.channel_id.map(|channel| (channel, route)))
        .collect();
    let edited_ids: Vec<Id> = those_edited.iter().map(|row| row.id).collect();
    let counts = pages::enabled_route_counts(conn, &edited_ids).await?;
    let mut shown = Vec::with_capacity(channels.len());
    for (row, can_edit) in channels.iter().zip(editable) {
        let route = routes.remove(&row.id);
        let declared = notifiers().get(&row.kind).map(|each| each.describe());
        let needs = match &declared {
            Some(declared) => route_logic::needs(conn, declared, &person, route.as_ref()).await?,
            None => None,
        };
        let waiting_count = match &route {
            Some(route) => route_rows::waiting_count(conn, route.id).await?,
            None => 0,
        };
        let link = match &route {
            Some(route) => own_link(conn, route, ctx_now(principal)).await?,
            None => None,
        };
        shown.push(views::channel_view(ChannelFacts {
            row,
            owner: views::owner_view(row, &orgs),
            can_edit,
            secrets_set: secrets.remove(&row.id).unwrap_or_default(),
            enabled_route_count: counts.get(&row.id).copied().unwrap_or(0),
            route: views::route_view(views::RouteFacts {
                channel_id: Some(row.id),
                channel: Some(views::route_channel_of(row, &orgs)),
                route: route.as_ref(),
                needs,
                waiting_count,
                link,
            }),
        }));
    }
    Ok(shown)
}

/// The moment the caller's request is answered at.
fn ctx_now(principal: &Principal) -> chrono::DateTime<chrono::Utc> {
    principal.now().get()
}

/// The link of the caller's own route that waits to be pressed, while it is good: the code is
/// kept sealed for its person alone until it lapses.
pub(super) async fn own_link(
    conn: &mut Conn,
    route: &route_rows::RouteRow,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Option<orch_api_contract::notifications::NotificationRouteLink>> {
    let (Some(expires_at), Some(channel)) = (route.link_expires_at, route.channel_id) else {
        return Ok(None);
    };
    if expires_at <= now {
        return Ok(None);
    }
    let Some(row) = rows::channel(conn, channel).await? else {
        return Ok(None);
    };
    let master = master_key()?;
    let url = orch_worker_core::bot_links::kept_url(conn, master, notifiers(), &row, route.id)
        .await
        .map_err(internal)?;
    Ok(url.map(
        |url| orch_api_contract::notifications::NotificationRouteLink {
            url,
            expires_at: orch_types::Time::new(expires_at),
        },
    ))
}

/// The ways out that can reach the caller, each with their own route: the
/// system's, those of the root orgs they belong to, and their own. With an
/// org, for whoever holds `settings.manage` on that root org: that org's, to
/// look after.
pub async fn list(
    ctx: Ctx,
    query: NotificationChannelListQuery,
    (): (),
) -> ApiResult<Vec<NotificationChannel>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let channels = match query.org_id {
        Some(org) => {
            authorization::require(
                &mut conn,
                &principal,
                org,
                exact(Permission::SettingsManage),
                Act::Use,
                Policy::default(),
            )
            .await?;
            root_org(&mut conn, org, &ctx).await?;
            pages::channels_of_org(&mut conn, org).await?
        }
        None => {
            let mut reaching = pages::channels_reaching(&mut conn, principal.user()).await?;
            // Where the system's mail server may not be used by an org that has none of its own,
            // it is no way out for the people of any org: there is no such channel in their list.
            // The admin who set it up is shown it all the same.
            if !principal.is_system_admin()
                && !channels::mail_may_be_shared(&mut conn)
                    .await
                    .map_err(internal)?
            {
                reaching.retain(|row| !(row.is_the_systems() && row.kind == "email"));
            }
            reaching
        }
    };
    views_for(&mut conn, &principal, &channels).await
}

/// Makes sure the node is a root org: nothing else has channels of its own.
async fn root_org(conn: &mut Conn, org: Id, ctx: &Ctx) -> ApiResult<()> {
    let detail = hierarchy::details(conn, &[org], ctx.now)
        .await?
        .into_iter()
        .next()
        .filter(|detail| detail.kind == "org" && detail.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    if detail.parent_id.is_some() {
        return Err(ApiError::invalid(
            "id",
            "is an org inside another: its people are reached by the channels of its root org",
        ));
    }
    Ok(())
}

/// Whether the system, or the root org, has a channel of the kind already.
async fn already_has(conn: &mut Conn, maker: Maker, kind: &str) -> ApiResult<bool> {
    Ok(match maker {
        Maker::System => !rows::system_channels_of(conn, kind).await?.is_empty(),
        Maker::Org(org) => rows::org_channel(conn, org, kind).await?.is_some(),
        Maker::Person(_) => false,
    })
}

/// The refusal of a second mail server or bot.
fn has_one_already(kind: &str) -> ApiError {
    ApiError::new(
        ErrorCode::ConflictExists,
        format!("there is a {kind} channel already; change that one"),
    )
}

/// What a request to set a channel up gives.
struct Making<'a> {
    maker: Maker,
    kind: &'a str,
    name: &'a str,
    config: &'a Json,
    secrets: Option<&'a BTreeMap<String, String>>,
    templates: Option<&'a TemplatesByLanguage>,
    enabled: Option<bool>,
}

/// Sets a channel up: looks at what was given and at the far side, then keeps
/// the channel, its sealed secrets and its line in the audit trail in one
/// transaction.
async fn make(ctx: &Ctx, making: Making<'_>) -> ApiResult<NotificationChannel> {
    let kind = making.kind;
    let notifier = notifiers()
        .get(kind)
        .ok_or_else(|| ApiError::invalid("kind", &format!("there is no notifier named {kind}")))?;
    let descriptor = notifier.describe();
    if !descriptor.offered_to(making.maker.owner()) {
        return Err(ApiError::invalid(
            "kind",
            &format!("{kind} cannot be set up for this"),
        ));
    }
    let name = checked_name(making.name)?;
    let answer = answer_of(&descriptor, making.config, making.secrets)?;
    let (overrides, stored_templates) = templates_of(making.templates)?;
    if descriptor.is_single() {
        // Said before the far side is asked anything: a second mail server or
        // bot is refused whatever it would have answered. The same is kept
        // again below, in the transaction that makes the channel.
        let mut conn = ctx.app.db.acquire().await?;
        if already_has(&mut conn, making.maker, kind).await? {
            return Err(has_one_already(kind));
        }
    }
    let allowed = match making.maker {
        Maker::System => None,
        Maker::Org(_) | Maker::Person(_) => {
            let mut conn = ctx.app.db.acquire().await?;
            Some(outbound_hosts(&mut conn).await.map_err(internal)?)
        }
    };
    // The look at the far side is made before a transaction begins: a server
    // may take its time to answer.
    let checked = check_before_save(
        notifiers(),
        &Given {
            kind,
            target: Target::Person,
            fields: &answer,
            templates: &overrides,
        },
        allowed,
    )
    .await
    .map_err(|fault| save_fault(Some(&descriptor), fault))?;
    let last_error = match &checked.saved {
        Saved::Refused(error) => return Err(refused(error)),
        Saved::KeptWithError(error) => Some(error.message.clone()),
        Saved::Kept(_) => None,
    };
    let values = checked.values_to_store();
    let master = if checked.config.secrets.is_empty() {
        None
    } else {
        Some(master_key()?)
    };
    let id = orch_types::new_id();
    let (org_id, user_id) = match making.maker {
        Maker::System => (None, None),
        Maker::Org(org) => (Some(org), None),
        Maker::Person(person) => (None, Some(person)),
    };

    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let mut current = ctx.clone();
    current.now = guard.now().get();
    let ctx = &current;
    let me = guard.user().id();
    match making.maker {
        Maker::System if !guard.user().is_system_admin() => {
            return Err(refusals::system_admin_only(
                "only a system admin sets up a channel of the system",
            ));
        }
        Maker::Org(org) => {
            root_org(&mut tx, org, ctx).await?;
            authorization::require(
                &mut tx,
                guard.principal(),
                org,
                exact(Permission::SettingsManage),
                Act::Use,
                Policy::default(),
            )
            .await?;
        }
        Maker::Person(person) if person != me => return Err(ApiError::not_found()),
        Maker::System | Maker::Person(_) => {}
    }
    if descriptor.is_single() {
        // A mail server and a bot: one for the system and one for each root
        // org. The schema keeps the built-in kinds to that; this keeps the
        // ones that were installed to it too.
        if making.maker == Maker::System {
            rows::lock_system_channels(&mut tx, kind).await?;
        }
        if already_has(&mut tx, making.maker, kind).await? {
            return Err(has_one_already(kind));
        }
    }
    rows::insert_channel(
        &mut tx,
        &NewChannel {
            id,
            org_id,
            user_id,
            kind,
            name: &name,
            config: &values,
            templates: &stored_templates,
            enabled: making.enabled.unwrap_or(true),
            created_by: me,
            at: ctx.now,
        },
    )
    .await?;
    if let Some(error) = &last_error {
        rows::record_channel_error(&mut tx, id, error).await?;
    }
    let mut kept: Vec<String> = Vec::new();
    if let Some(master) = master {
        for (secret_name, secret) in &checked.config.secrets {
            let sealed = seal_secret(master, owner::CHANNEL, id, secret_name, secret.expose())
                .map_err(|_| internal("a secret could not be sealed"))?;
            settings::put_secret(
                &mut tx,
                owner::CHANNEL,
                id,
                secret_name,
                &sealed,
                Some(me),
                ctx.now,
            )
            .await?;
            kept.push(secret_name.clone());
        }
    }
    if !matches!(making.maker, Maker::Person(_)) {
        write_audit(
            &mut tx,
            ctx,
            Audited {
                action: orch_db::audit::action!(AuditAction::ChannelCreated),
                maker: making.maker,
                channel: id,
                kind,
                name: &name,
                values: &values,
                affected: None,
                secrets: &kept,
            },
        )
        .await?;
    }
    let row = rows::channel(&mut tx, id)
        .await?
        .ok_or_else(|| internal("the channel vanished"))?;
    tx.commit().await.map_err(DbError::from)?;
    one_view(ctx, &row).await
}

/// What the audit line of a channel says of it.
struct Audited<'a> {
    action: AuditAction,
    maker: Maker,
    channel: Id,
    kind: &'a str,
    name: &'a str,
    values: &'a Json,
    secrets: &'a [String],
    /// For a removal: how many people had a route through it on, never who they were.
    affected: Option<i64>,
}

/// The line in the audit trail of a channel of the system or of a root org:
/// which kind and which fields, never a value of a secret.
async fn write_audit(conn: &mut Conn, ctx: &Ctx, said: Audited<'_>) -> ApiResult<()> {
    let whose = match said.maker {
        Maker::System => "of the system",
        Maker::Org(_) => "of a root org",
        Maker::Person(_) => "of a person",
    };
    let verb = match said.action {
        AuditAction::ChannelCreated => "set up",
        AuditAction::ChannelUpdated => "changed",
        _ => "removed",
    };
    let summary = format!("{verb} the {} channel \"{}\" {whose}", said.kind, said.name);
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        said.action.as_str(),
        said.action.object_kind().unwrap_or("channel"),
        Some(said.channel),
        &summary,
    );
    entry.history = said.action.is_history_line();
    if let Maker::Org(org) = said.maker {
        entry.node_id = Some(org);
        entry.org_id = Some(org);
    }
    entry.details = json!({
        "kind": said.kind,
        "fields": said.values.as_object().map(|fields| fields.keys().cloned().collect::<Vec<_>>()),
        "secrets": said.secrets,
        "affected_routes": said.affected,
    });
    audit::append(conn, &entry, &sign_in::origin(ctx), ctx.now).await?;
    Ok(())
}

/// One channel as the caller reads it.
async fn one_view(ctx: &Ctx, row: &ChannelRow) -> ApiResult<NotificationChannel> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    views_for(&mut conn, &principal, std::slice::from_ref(row))
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)
}

/// Sets up a channel of the system: its mail server, its bot, a webhook, push.
pub async fn create_system(
    ctx: Ctx,
    (): (),
    body: NotificationChannelCreateBody,
) -> ApiResult<NotificationChannel> {
    if !ctx.user()?.is_system_admin() {
        return Err(refusals::system_admin_only(
            "only a system admin sets up a channel of the system",
        ));
    }
    make(
        &ctx,
        Making {
            maker: Maker::System,
            kind: &body.kind,
            name: &body.name,
            config: &body.config,
            secrets: body.secrets.as_ref(),
            templates: body.templates.as_ref(),
            enabled: body.enabled,
        },
    )
    .await
}

/// Sets up a channel of a root org, for its members; a second mail server or
/// bot is refused.
pub async fn create_org(
    ctx: Ctx,
    (): (),
    body: NotificationChannelCreateBody,
) -> ApiResult<NotificationChannel> {
    let org = ctx.id("id")?;
    {
        let mut conn = ctx.app.db.acquire().await?;
        root_org(&mut conn, org, &ctx).await?;
    }
    make(
        &ctx,
        Making {
            maker: Maker::Org(org),
            kind: &body.kind,
            name: &body.name,
            config: &body.config,
            secrets: body.secrets.as_ref(),
            templates: body.templates.as_ref(),
            enabled: body.enabled,
        },
    )
    .await
}

/// Sets up a webhook of the caller's own.
pub async fn create_own(
    ctx: Ctx,
    (): (),
    body: MeNotificationChannelCreateBody,
) -> ApiResult<NotificationChannel> {
    let config = serde_json::to_value(&body.config).map_err(internal)?;
    let secrets = body
        .secrets
        .as_ref()
        .map(|secret| BTreeMap::from([("secret".to_owned(), secret.secret.clone())]));
    make(
        &ctx,
        Making {
            maker: Maker::Person(ctx.user()?.id()),
            kind: "webhook",
            name: &body.name,
            config: &config,
            secrets: secrets.as_ref(),
            templates: body.templates.as_ref(),
            enabled: None,
        },
    )
    .await
}

/// The channel `id`, or not found.
async fn channel_of(conn: &mut Conn, ctx: &Ctx) -> ApiResult<ChannelRow> {
    rows::channel(conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// What is looked at again when some values of a channel or a destination
/// change.
pub(super) struct Recheck<'a> {
    /// The notifier.
    pub kind: &'a str,
    /// What one of it is.
    pub target: Target,
    /// The values it has.
    pub stored: &'a Json,
    /// The secrets it has, opened for this call.
    pub current: &'a orch_notifier_contract::delivery::Config,
    /// The values given, which replace the ones it has.
    pub config: Option<&'a Json>,
    /// The secrets given: each replaces the one it names, and an empty one
    /// takes the secret away.
    pub secrets: Option<&'a BTreeMap<String, String>>,
    /// Its texts, as they will be.
    pub texts: &'a BTreeMap<String, TemplateOverride>,
    /// The private hosts it may reach; none for the system's.
    pub allowed: Option<Vec<String>>,
}

/// What looking again came to.
pub(super) struct Rechecked {
    /// The values to keep, with what the far side said of itself.
    pub values: Json,
    /// Why the far side could not be reached, where it could not.
    pub unreached: Option<String>,
    /// The secrets to seal and keep.
    pub put: Vec<(String, String)>,
    /// The secrets to take away.
    pub remove: Vec<String>,
}

/// Looks at a channel or a destination as it will be once changed: the values
/// given, or else those it has of the fields the notifier declares, and its
/// secrets, those it has and the ones given over them. What the far side
/// refuses for good refuses the change.
pub(super) async fn recheck(again: Recheck<'_>) -> ApiResult<Rechecked> {
    let notifier = notifiers().get(again.kind).ok_or_else(|| {
        ApiError::invalid(
            "kind",
            &format!("there is no notifier named {}", again.kind),
        )
    })?;
    let descriptor = notifier.describe();
    let is_secret = |name: &str| {
        descriptor
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.kind.is_secret())
    };
    let Json::Object(mut answer) = again.config.unwrap_or(again.stored).clone() else {
        return Err(ApiError::invalid(
            "config",
            "is an object of the notifier's fields",
        ));
    };
    if again.config.is_none() {
        // The values it has include what the far side said of itself when
        // they were saved (a bot's name), which no field asks for.
        answer.retain(|member, _| is_secret(member) == Some(false));
    }
    if let Some(name) = answer.keys().find(|member| is_secret(member) == Some(true)) {
        return Err(ApiError::invalid(
            &format!("config.{name}"),
            "is a secret: give it under secrets",
        ));
    }
    for (name, secret) in &again.current.secrets {
        answer.insert(name.clone(), Json::String(secret.expose().to_owned()));
    }
    let (mut put, mut remove) = (Vec::new(), Vec::new());
    for (name, value) in again.secrets.into_iter().flatten() {
        if is_secret(name) != Some(true) {
            return Err(ApiError::invalid(
                &format!("secrets.{name}"),
                "is not a secret field of this notifier",
            ));
        }
        if value.is_empty() {
            answer.remove(name);
            remove.push(name.clone());
        } else {
            answer.insert(name.clone(), Json::String(value.clone()));
            put.push((name.clone(), value.clone()));
        }
    }
    let checked = check_before_save(
        notifiers(),
        &Given {
            kind: again.kind,
            target: again.target,
            fields: &Json::Object(answer),
            templates: again.texts,
        },
        again.allowed,
    )
    .await
    .map_err(|fault| save_fault(Some(&descriptor), fault))?;
    let unreached = match &checked.saved {
        Saved::Refused(error) => return Err(refused(error)),
        Saved::KeptWithError(error) => Some(error.message.clone()),
        Saved::Kept(_) => None,
    };
    Ok(Rechecked {
        values: checked.values_to_store(),
        unreached,
        put,
        remove,
    })
}

/// Checks the texts of a channel or a destination that is changed in nothing
/// else, as they are saved.
pub(super) fn check_texts_of(
    kind: &str,
    texts: &BTreeMap<String, TemplateOverride>,
) -> ApiResult<()> {
    if let Some(notifier) = notifiers().get(kind) {
        let descriptor = notifier.describe();
        orch_notifier_contract::templates::check_texts(texts, descriptor.body)
            .map_err(|list| save_fault(Some(&descriptor), SaveFault::Texts(list)))?;
    }
    Ok(())
}

/// Changes a channel. What is left out stays; the secrets named are replaced
/// and the others kept, and an empty one takes the secret away.
pub async fn update(
    mut ctx: Ctx,
    (): (),
    body: NotificationChannelUpdateBody,
) -> ApiResult<NotificationChannel> {
    let row = {
        let mut conn = ctx.app.db.acquire().await?;
        let row = channel_of(&mut conn, &ctx).await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        require_change(&mut conn, &principal, &row).await?;
        row
    };
    let name = body.name.as_deref().map(checked_name).transpose()?;
    let (overrides, stored_templates) = templates_of(body.templates.as_ref())?;
    let mut master = None;
    let mut again: Option<Rechecked> = None;
    if body.config.is_some() || body.secrets.is_some() {
        let master_key = master_key()?;
        master = Some(master_key);
        let (current, allowed) = {
            let mut conn = ctx.app.db.acquire().await?;
            let current = config_of_channel(&mut conn, master_key, &row)
                .await
                .map_err(internal)?;
            let allowed = if row.is_the_systems() {
                None
            } else {
                Some(outbound_hosts(&mut conn).await.map_err(internal)?)
            };
            (current, allowed)
        };
        again = Some(
            recheck(Recheck {
                kind: &row.kind,
                target: Target::Person,
                stored: &row.config,
                current: &current,
                config: body.config.as_ref(),
                secrets: body.secrets.as_ref(),
                texts: &overrides,
                allowed,
            })
            .await?,
        );
    } else if body.templates.is_some() {
        check_texts_of(&row.kind, &overrides)?;
    }

    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let row = channel_of(&mut tx, &ctx).await?;
    require_change(&mut tx, guard.principal(), &row).await?;
    let changed = rows::update_channel(
        &mut tx,
        row.id,
        &ChannelChange {
            name: name.as_deref(),
            config: again.as_ref().map(|again| &again.values),
            templates: body.templates.as_ref().map(|_| &stored_templates),
            enabled: body.enabled,
        },
        ctx.now,
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    let me = guard.user().id();
    let mut sealed_names: Vec<String> = Vec::new();
    if let Some(again) = &again {
        match &again.unreached {
            Some(error) => rows::record_channel_error(&mut tx, row.id, error).await?,
            None => pages::clear_channel_error(&mut tx, row.id).await?,
        }
        // The fields may name another bot than the one whose links were read.
        rows::set_link_cursor(&mut tx, row.id, None).await?;
        for (secret_name, value) in &again.put {
            let master = master.ok_or_else(|| internal("the key that seals secrets is missing"))?;
            let sealed = seal_secret(master, owner::CHANNEL, row.id, secret_name, value)
                .map_err(|_| internal("a secret could not be sealed"))?;
            settings::put_secret(
                &mut tx,
                owner::CHANNEL,
                row.id,
                secret_name,
                &sealed,
                Some(me),
                ctx.now,
            )
            .await?;
            sealed_names.push(secret_name.clone());
        }
        for secret_name in &again.remove {
            settings::delete_secret(&mut tx, owner::CHANNEL, row.id, secret_name).await?;
        }
    }
    let maker = match (row.org_id, row.user_id) {
        (Some(org), _) => Maker::Org(org),
        (None, Some(person)) => Maker::Person(person),
        (None, None) => Maker::System,
    };
    if !matches!(maker, Maker::Person(_)) {
        write_audit(
            &mut tx,
            &ctx,
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
    let after = rows::channel(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(DbError::from)?;
    one_view(&ctx, &after).await
}

/// Deletes a channel, which ends every route through it: the people who had
/// one are told on their screens.
pub async fn delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let row = channel_of(&mut tx, &ctx).await?;
    require_change(&mut tx, guard.principal(), &row).await?;
    let routed = pages::people_routed_through(&mut tx, row.id).await?;
    let affected = pages::enabled_route_counts(&mut tx, &[row.id])
        .await?
        .get(&row.id)
        .copied()
        .unwrap_or(0);
    if !rows::delete_channel(&mut tx, row.id, ctx.now).await? {
        return Err(ApiError::not_found());
    }
    let maker = match (row.org_id, row.user_id) {
        (Some(org), _) => Maker::Org(org),
        (None, Some(person)) => Maker::Person(person),
        (None, None) => Maker::System,
    };
    if !matches!(maker, Maker::Person(_)) {
        write_audit(
            &mut tx,
            &ctx,
            Audited {
                action: orch_db::audit::action!(AuditAction::ChannelDeleted),
                maker,
                channel: row.id,
                kind: &row.kind,
                name: &row.name,
                values: &row.config,
                affected: Some(affected),
                secrets: &[],
            },
        )
        .await?;
    }
    for person in routed {
        route_rows::tell_route_changed(&mut tx, person, row.id).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Sends one sample message through a channel, to the caller as their route
/// reaches them or, for a mail server, to the address given, and says what the
/// far side answered when it did not take it.
///
/// The message is made under the caller's current authority, and that
/// transaction ends before the far side is called: a slow server holds no
/// connection. What it answered is kept afterwards, under a fresh check.
pub async fn test(
    mut ctx: Ctx,
    (): (),
    body: NotificationChannelTestBody,
) -> ApiResult<NotificationTestReply> {
    let (prepared, editor, channel, route_id) = {
        let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
        ctx.now = guard.now().get();
        let row = channel_of(&mut conn, &ctx).await?;
        let principal = guard.principal();
        let me = principal.user();
        let mut managed = HashMap::new();
        let editor = may_change(&mut conn, principal, &row, &mut managed).await?;
        let routed = pages::is_routed(&mut conn, me, row.id).await?;
        if row.user_id.is_some_and(|person| person != me) {
            return Err(ApiError::not_found());
        }
        if !editor && !routed {
            require_change(&mut conn, principal, &row).await?;
            return Err(not_for_the_caller(row.id));
        }
        if body.to.is_some() && !editor {
            return Err(ApiError::invalid(
                "to",
                "is given by whoever looks after the channel; for you it writes to you",
            ));
        }
        let caller = people::recipient(&mut conn, me)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let route = route_rows::route_for(&mut conn, me, row.id).await?;
        let master = master_key()?;
        let around = surroundings(&ctx);
        let prepared = channels::prepare_test(
            &mut conn,
            &toolbox(master, &around),
            &TestFor {
                channel: &row,
                caller: &caller,
                route_address: route.as_ref().map(|route| &route.address),
                to: body.to.as_deref(),
                device: body.device_id,
            },
            ctx.now,
        )
        .await
        .map_err(|error| match error {
            channels::ChannelError::UnknownKind(kind) => {
                ApiError::invalid("kind", &format!("there is no notifier named {kind}"))
            }
            other => internal(other),
        })?;
        conn.commit().await.map_err(DbError::from)?;
        (prepared, editor, row.id, route.map(|route| route.id))
    };
    let outcome = match prepared {
        channels::TestPrepared::Done(outcome) => outcome,
        channels::TestPrepared::Ready(ready) => channels::run_test(ready).await,
    };
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let now = guard.now().get();
    // What the far side answered is shown on the channel to whoever still looks after it; a
    // member's own trouble is not the channel's. A test that was taken says when the caller's
    // route last reached them, and sends nothing that waited and turns nothing on.
    if let Some(row) = rows::channel(&mut conn, channel).await? {
        let mut managed = HashMap::new();
        if editor && may_change(&mut conn, guard.principal(), &row, &mut managed).await? {
            match &outcome.error {
                Some(error) => rows::record_channel_error(&mut conn, row.id, error).await?,
                None => rows::record_channel_ok(&mut conn, row.id, now).await?,
            }
        }
        if let (Some(route), true) = (route_id, outcome.ok) {
            route_rows::record_test_taken(&mut conn, route, now).await?;
        }
    }
    conn.commit().await.map_err(DbError::from)?;
    Ok(NotificationTestReply {
        ok: outcome.ok,
        error: outcome.error,
    })
}

/// A refusal of something only whoever looks after a channel, or has a route
/// through it, may do.
fn not_for_the_caller(channel: Id) -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "only whoever looks after the channel, or has a route through it, tries it",
    )
    .with("required", "notification.channel.test")
    .with(
        "object",
        json!({ "kind": "notification_channel", "id": channel }),
    )
    .with("requestable", false)
    .with("override", false)
    .with(
        "missing",
        Vec::<orch_api_contract::access::MissingAccess>::new(),
    )
}
