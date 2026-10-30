//! The destinations of a node: where what happens at an org, a project or a topic is sent, for nobody in particular, such as a team's room in a chat tool or a build system.
//!
//! Whoever holds `settings.manage` on the node adds, changes, removes and
//! tries them. A destination is made from a notifier that can be one, with its
//! fields checked as a channel's are and its secrets sealed and never
//! returned; it is always held to the list of private hosts, since it is set
//! up by whoever manages a node and not by the system. What it is sent is
//! decided when something happens (`orch_engine::notifications`): the kinds
//! that may be sent to a destination, at the level it has.

use super::channels::{
    Recheck, Rechecked, answer_of, check_texts_of, checked_name, recheck, refused, save_fault,
};
use super::views::{self, level_word};
use super::{master_key, notifiers, surroundings, toolbox};
use crate::middleware::authorization::{self, exact};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::routes::nodes::view;
use crate::sign_in;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::notifications::{
    Destination, DestinationLevel, DestinationUpdateBody, NodeDestinationCreateBody,
    NotificationKind, NotificationTestReply,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::hierarchy::{self, NodeDetail};
use orch_db::queries::notifications::channels::{
    self as rows, DestinationRow, NewDestination, owner,
};
use orch_db::queries::notifications::pages::{self, DestinationChange};
use orch_db::queries::settings;
use orch_db::{Conn, DbError};
use orch_engine::notifications::destination_reach;
use orch_notifier_contract::delivery::Saved;
use orch_notifier_contract::descriptor::Target;
use orch_types::Id;
use orch_worker_core::channels::{self, config_of_destination, outbound_hosts, seal_secret};
use serde_json::json;

/// The channel a destination is made from unless the request says another.
const DEFAULT_CHANNEL: &str = "webhook";

/// A node that can have destinations: an org at any level, a project or a topic.
async fn destined_node(conn: &mut Conn, ctx: &Ctx, node: Id) -> ApiResult<NodeDetail> {
    let detail = hierarchy::details(conn, &[node], ctx.now)
        .await?
        .into_iter()
        .next()
        .filter(|detail| detail.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    if detail.kind == "thread" {
        return Err(ApiError::invalid(
            "id",
            "is a thread: destinations are set at an org, a project or a topic",
        ));
    }
    Ok(detail)
}

/// The words of the kinds a destination was asked to be sent, which must be
/// kinds a destination is ever sent.
fn kinds_words(kinds: &[String]) -> ApiResult<Vec<String>> {
    let mut words: Vec<String> = Vec::new();
    for word in kinds {
        let kind: Option<NotificationKind> = views::kind_of(word);
        // An announcement is the kind that depends on its severity: it is sent
        // to a destination at one level or the other.
        let takes = kind.is_some_and(|kind| destination_reach(kind, true).is_some());
        if !takes {
            return Err(ApiError::invalid(
                "kinds",
                &format!("{word:?} is not a kind that is sent to a destination"),
            ));
        }
        if !words.contains(word) {
            words.push(word.clone());
        }
    }
    Ok(words)
}

/// The destinations as the caller reads them, each at the node it is set at.
async fn views_of(
    conn: &mut Conn,
    ctx: &Ctx,
    rows: &[DestinationRow],
) -> ApiResult<Vec<Destination>> {
    let principal = authorization::principal(ctx, conn).await?;
    let nodes: Vec<Id> = rows.iter().map(|row| row.node_id).collect();
    let refs = view::refs(conn, &principal, &nodes, Policy::default()).await?;
    let owners: Vec<(&str, Id)> = rows
        .iter()
        .map(|row| (owner::DESTINATION, row.id))
        .collect();
    let mut names: std::collections::HashMap<Id, Vec<String>> = std::collections::HashMap::new();
    for secret in settings::secret_names_of(conn, &owners).await? {
        names.entry(secret.owner_id).or_default().push(secret.name);
    }
    rows.iter()
        .map(|row| {
            let node = refs
                .get(&row.node_id)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            let mut secrets = names.remove(&row.id).unwrap_or_default();
            secrets.sort();
            Ok(views::destination_view(row, node, secrets))
        })
        .collect()
}

/// Lists the destinations of a node.
pub async fn list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Destination>> {
    let node = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    destined_node(&mut conn, &ctx, node).await?;
    let found = pages::destinations_of(&mut conn, node).await?;
    views_of(&mut conn, &ctx, &found).await
}

/// Sets up a destination at a node, which is an org at any level, a project
/// or a topic.
pub async fn create(
    mut ctx: Ctx,
    (): (),
    body: NodeDestinationCreateBody,
) -> ApiResult<Destination> {
    let node = ctx.id("id")?;
    let detail = {
        let mut conn = ctx.app.db.acquire().await?;
        destined_node(&mut conn, &ctx, node).await?
    };
    let kind = body.channel.as_deref().unwrap_or(DEFAULT_CHANNEL);
    let notifier = notifiers().get(kind).ok_or_else(|| {
        ApiError::invalid("channel", &format!("there is no notifier named {kind}"))
    })?;
    let descriptor = notifier.describe();
    if !descriptor.is_destination() {
        return Err(ApiError::invalid(
            "channel",
            &format!("{kind} cannot be set up for this"),
        ));
    }
    let name = checked_name(&body.name)?;
    let answer = answer_of(&descriptor, &body.config, body.secrets.as_ref())?;
    let kinds = kinds_words(body.kinds.as_deref().unwrap_or_default())?;
    let level = level_word(body.level.unwrap_or(DestinationLevel::Important));
    let (overrides, stored_templates) = match body.templates.as_ref() {
        Some(given) => (
            views::overrides_of(given).map_err(ApiError::internal)?,
            serde_json::to_value(given).map_err(ApiError::internal)?,
        ),
        None => (Default::default(), json!({})),
    };
    let allowed = {
        let mut conn = ctx.app.db.acquire().await?;
        outbound_hosts(&mut conn)
            .await
            .map_err(ApiError::internal)?
    };
    let checked = channels::check_before_save(
        notifiers(),
        &channels::Given {
            kind,
            target: Target::Destination,
            fields: &answer,
            templates: &overrides,
        },
        Some(allowed),
    )
    .await
    .map_err(|fault| save_fault(Some(&descriptor), fault))?;
    let unreached = match &checked.saved {
        Saved::Refused(error) => return Err(refused(error)),
        Saved::KeptWithError(error) => Some(error.message.clone()),
        Saved::Kept(_) => None,
    };
    let values = checked.values_to_store();
    let id = orch_types::new_id();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let me = guard.user().id();
    destined_node(&mut tx, &ctx, node).await?;
    rows::insert_destination(
        &mut tx,
        &NewDestination {
            id,
            node_id: node,
            channel: kind,
            name: &name,
            config: &values,
            templates: &stored_templates,
            kinds: &kinds,
            level,
            enabled: body.enabled.unwrap_or(true),
            created_by: me,
            at: ctx.now,
        },
    )
    .await?;
    if let Some(error) = &unreached {
        rows::record_destination_error(&mut tx, id, error).await?;
    }
    let mut kept: Vec<String> = Vec::new();
    for (secret_name, secret) in &checked.config.secrets {
        let sealed = seal_secret(
            master_key()?,
            owner::DESTINATION,
            id,
            secret_name,
            secret.expose(),
        )
        .map_err(|_| ApiError::internal("a secret could not be sealed"))?;
        settings::put_secret(
            &mut tx,
            owner::DESTINATION,
            id,
            secret_name,
            &sealed,
            Some(me),
            ctx.now,
        )
        .await?;
        kept.push(secret_name.clone());
    }
    write_audit(
        &mut tx,
        &ctx,
        Audited {
            action: orch_db::audit::action!(AuditAction::DestinationCreated),
            node: &detail,
            destination: id,
            channel: kind,
            name: &name,
            values: &values,
            secrets: &kept,
        },
    )
    .await?;
    let row = rows::destination(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(DbError::from)?;
    one_view(&ctx, &row).await
}

/// What the audit line of a destination says of it.
struct Audited<'a> {
    action: AuditAction,
    node: &'a NodeDetail,
    destination: Id,
    channel: &'a str,
    name: &'a str,
    values: &'a serde_json::Value,
    secrets: &'a [String],
}

/// The line in the audit trail of a destination: which kind of notifier and
/// which fields, never a value of a secret.
async fn write_audit(conn: &mut Conn, ctx: &Ctx, said: Audited<'_>) -> ApiResult<()> {
    let verb = match said.action {
        AuditAction::DestinationCreated => "added",
        AuditAction::DestinationUpdated => "changed",
        _ => "removed",
    };
    let summary = format!(
        "{verb} the {} destination \"{}\" of {}",
        said.channel, said.name, said.node.name
    );
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        said.action.as_str(),
        said.action.object_kind().unwrap_or("destination"),
        Some(said.destination),
        &summary,
    );
    entry.history = said.action.is_history_line();
    entry.node_id = Some(said.node.id);
    entry.org_id = Some(said.node.org_id);
    entry.details = json!({
        "channel": said.channel,
        "fields": said.values.as_object().map(|fields| fields.keys().cloned().collect::<Vec<_>>()),
        "secrets": said.secrets,
    });
    audit::append(conn, &entry, &sign_in::origin(ctx), ctx.now).await?;
    Ok(())
}

async fn one_view(ctx: &Ctx, row: &DestinationRow) -> ApiResult<Destination> {
    let mut conn = ctx.app.db.acquire().await?;
    views_of(&mut conn, ctx, std::slice::from_ref(row))
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)
}

/// The destination the path names, for a caller who may manage its node.
async fn managed(ctx: &Ctx) -> ApiResult<(DestinationRow, NodeDetail)> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    managed_in(&mut conn, ctx, &principal).await
}

async fn managed_in(
    conn: &mut Conn,
    ctx: &Ctx,
    principal: &Principal,
) -> ApiResult<(DestinationRow, NodeDetail)> {
    let row = rows::destination(conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    authorization::require(
        conn,
        principal,
        row.node_id,
        exact(Permission::SettingsManage),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let node = hierarchy::details(conn, &[row.node_id], ctx.now)
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    Ok((row, node))
}

/// Changes a destination. What is left out stays; the secrets named are
/// replaced and the others kept, and an empty one takes the secret away.
pub async fn update(mut ctx: Ctx, (): (), body: DestinationUpdateBody) -> ApiResult<Destination> {
    let (row, _) = managed(&ctx).await?;
    let name = body.name.as_deref().map(checked_name).transpose()?;
    let kinds = body.kinds.as_deref().map(kinds_words).transpose()?;
    let level = body.level.map(level_word);
    let (overrides, stored_templates) = match body.templates.as_ref() {
        Some(given) => (
            views::overrides_of(given).map_err(ApiError::internal)?,
            serde_json::to_value(given).map_err(ApiError::internal)?,
        ),
        None => (Default::default(), json!({})),
    };
    let mut master = None;
    let mut again: Option<Rechecked> = None;
    if body.config.is_some() || body.secrets.is_some() {
        let master_key = master_key()?;
        master = Some(master_key);
        let (current, allowed) = {
            let mut conn = ctx.app.db.acquire().await?;
            let current = config_of_destination(&mut conn, master_key, &row)
                .await
                .map_err(ApiError::internal)?;
            let allowed = outbound_hosts(&mut conn)
                .await
                .map_err(ApiError::internal)?;
            (current, allowed)
        };
        again = Some(
            recheck(Recheck {
                kind: &row.channel,
                target: Target::Destination,
                stored: &row.config,
                current: &current,
                config: body.config.as_ref(),
                secrets: body.secrets.as_ref(),
                texts: &overrides,
                allowed: Some(allowed),
            })
            .await?,
        );
    } else if body.templates.is_some() {
        check_texts_of(&row.channel, &overrides)?;
    }

    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let me = guard.user().id();
    let (row, node) = managed_in(&mut tx, &ctx, guard.principal()).await?;
    let changed = pages::update_destination(
        &mut tx,
        row.id,
        &DestinationChange {
            name: name.as_deref(),
            config: again.as_ref().map(|again| &again.values),
            templates: body.templates.as_ref().map(|_| &stored_templates),
            kinds: kinds.as_deref(),
            level,
            enabled: body.enabled,
        },
        ctx.now,
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    let mut sealed_names: Vec<String> = Vec::new();
    if let Some(again) = &again {
        match &again.unreached {
            Some(error) => rows::record_destination_error(&mut tx, row.id, error).await?,
            None => pages::clear_destination_error(&mut tx, row.id).await?,
        }
        for (secret_name, value) in &again.put {
            let master = master
                .ok_or_else(|| ApiError::internal("the key that seals secrets is missing"))?;
            let sealed = seal_secret(master, owner::DESTINATION, row.id, secret_name, value)
                .map_err(|_| ApiError::internal("a secret could not be sealed"))?;
            settings::put_secret(
                &mut tx,
                owner::DESTINATION,
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
            settings::delete_secret(&mut tx, owner::DESTINATION, row.id, secret_name).await?;
        }
    }
    write_audit(
        &mut tx,
        &ctx,
        Audited {
            action: orch_db::audit::action!(AuditAction::DestinationUpdated),
            node: &node,
            destination: row.id,
            channel: &row.channel,
            name: &changed.name,
            values: &changed.config,
            secrets: &sealed_names,
        },
    )
    .await?;
    let after = rows::destination(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(DbError::from)?;
    one_view(&ctx, &after).await
}

/// Deletes a destination, with its sealed secrets and what waits to be sent it.
pub async fn delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let (row, node) = managed_in(&mut tx, &ctx, guard.principal()).await?;
    if !rows::delete_destination(&mut tx, row.id).await? {
        return Err(ApiError::not_found());
    }
    write_audit(
        &mut tx,
        &ctx,
        Audited {
            action: orch_db::audit::action!(AuditAction::DestinationDeleted),
            node: &node,
            destination: row.id,
            channel: &row.channel,
            name: &row.name,
            values: &row.config,
            secrets: &[],
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Delivers one sample notification to a destination now, and says what came
/// back. The notification is made under the caller's current authority, and
/// that transaction ends before the far side is called; what it answered is
/// kept afterwards, under a fresh check.
pub async fn test(mut ctx: Ctx, (): (), (): ()) -> ApiResult<NotificationTestReply> {
    let prepared = {
        let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
        ctx.now = guard.now().get();
        let (row, _) = managed_in(&mut conn, &ctx, guard.principal()).await?;
        let around = surroundings(&ctx);
        let prepared = channels::prepare_destination_test(
            &mut conn,
            &toolbox(master_key()?, &around),
            &row,
            ctx.now,
        )
        .await
        .map_err(|error| match error {
            channels::ChannelError::UnknownKind(kind) => {
                ApiError::invalid("channel", &format!("there is no notifier named {kind}"))
            }
            other => ApiError::internal(other),
        })?;
        conn.commit().await.map_err(DbError::from)?;
        prepared
    };
    let outcome = match prepared {
        channels::TestPrepared::Done(outcome) => outcome,
        channels::TestPrepared::Ready(ready) => channels::run_test(ready).await,
    };
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let now = guard.now().get();
    // Only while the caller still looks after it is what came back kept on it.
    let (row, _) = managed_in(&mut conn, &ctx, guard.principal()).await?;
    match &outcome.error {
        Some(error) => rows::record_destination_error(&mut conn, row.id, error).await?,
        None => rows::record_destination_ok(&mut conn, row.id, now).await?,
    }
    conn.commit().await.map_err(DbError::from)?;
    Ok(NotificationTestReply {
        ok: outcome.ok,
        error: outcome.error,
    })
}
