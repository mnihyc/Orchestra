//! Removing a definition keeps data in the native custody ledger before any physical cleanup runs.
use super::super::{PreparedRecords, report_sources::Book};
use super::*;
use crate::middleware::authorization::refusals;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::resource_facts::{DependentRelation, FactSubject, ResourceFacts};
use orch_authz::{
    Principal,
    resources::{Locating, LocationAsk, LocationKind},
};
use orch_db::{Conn, DbError, queries::resources::mounts as rows};
use orch_engine::{
    notifications::{self, NotificationOperation, OperationRecipient},
    resource_facts,
    storage::record,
};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    mount: Id,
    revision: i64,
    locations: BTreeMap<Id, Id>,
    kept: BTreeSet<Id>,
    lost: BTreeSet<Id>,
}

#[derive(Clone, sqlx::FromRow)]
struct Location {
    membership_id: Id,
    machine_id: Id,
    kind: String,
    source: String,
    storage: Option<String>,
    data_kind: Option<String>,
    last_bytes: Option<i64>,
    created_at: chrono::DateTime<chrono::Utc>,
}

pub(super) struct Prepared {
    pub snapshot: Snapshot,
    root: rows::MountRow,
    locations: Vec<Location>,
    rows: Vec<api::KeptRecord>,
    targets: HashMap<String, Target>,
    effects: ResourceFacts,
    environments: Vec<Id>,
    recipients: Vec<OperationRecipient>,
    visible_people: BTreeSet<Id>,
    notify: bool,
    purge_default: bool,
}

pub(super) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    request: &api::CleanupRequest,
    mut book: Option<&mut Book>,
) -> ApiResult<Prepared> {
    let object = request
        .resource
        .filter(|object| object.kind == ObjectKind::Mount)
        .ok_or_else(ApiError::not_found)?;
    let root = rows::lock_for_custody(conn, object.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let locations: Vec<Location> = sqlx::query_as("SELECT membership_id,machine_id,kind,source,storage,data_kind,last_bytes,created_at FROM mount_locations WHERE mount_id=$1 ORDER BY membership_id FOR UPDATE")
        .bind(root.id).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    sqlx::query("SELECT id FROM mount_kept_data WHERE mount_id=$1 ORDER BY id FOR UPDATE")
        .bind(root.id)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let kept = rows::kept(conn, &[root.id]).await?;
    let lost = rows::lost_locations(conn, root.id).await?;
    // Supplier withdrawal changes only this mount's references, including hidden consumers.
    // Acquire those native writers before the first effect, without detaching their running data.
    sqlx::query("SELECT node_id FROM settings WHERE key='env.mounts' AND value ? $1 ORDER BY node_id NULLS FIRST FOR UPDATE")
        .bind(root.id.to_string()).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    sqlx::query(
        "SELECT id FROM environments WHERE config->'env.mounts' ? $1 ORDER BY id FOR NO KEY UPDATE",
    )
    .bind(root.id.to_string())
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let snapshot = Snapshot {
        mount: root.id,
        revision: root.revision,
        locations: locations
            .iter()
            .map(|row| (row.membership_id, row.machine_id))
            .collect(),
        kept: kept.iter().map(|row| row.id).collect(),
        lost: lost.iter().map(|row| row.id).collect(),
    };
    let raw = resource_facts::dependents(
        conn,
        &[FactSubject::Resource { object }],
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::engine)?;
    let mut consequences = raw.clone();
    for facts in &mut consequences {
        facts.dependents.retain(|edge| {
            edge.relation == DependentRelation::ExplicitGiving
                || edge.relation == DependentRelation::Attachment
                    && edge
                        .object
                        .is_some_and(|object| object.kind == ObjectKind::Environment)
        });
    }
    let mut visibility =
        resource_facts::prepare_projection(conn, principal, Default::default(), &consequences)
            .await
            .map_err(ApiError::engine)?;
    if let Some(book) = book.as_deref_mut() {
        book.keep_object(
            conn,
            format!("mount-definition:{}", root.id),
            object,
            json!(snapshot),
        )
        .await?;
        let mut names = HashSet::new();
        for source in visibility.sources() {
            if book.name(conn, source).await? {
                names.insert((source.kind, source.id));
            }
        }
        visibility.restrict_to(&names);
        let system_name = principal.is_system_admin()
            && consequences
                .iter()
                .flat_map(|facts| &facts.dependents)
                .any(|edge| edge.visibility.is_none())
            && book.system_name()?;
        visibility.restrict_system_names(system_name);
        let machines = locations
            .iter()
            .map(|row| row.machine_id)
            .chain(kept.iter().map(|row| row.machine_id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        book.prepare_machines(conn, &machines).await?;
    }
    let effects = visibility
        .project(&consequences)
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let environments = raw
        .iter()
        .flat_map(|row| &row.dependents)
        .filter_map(|edge| {
            edge.object
                .filter(|object| object.kind == ObjectKind::Environment)
                .map(|object| object.id)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM runtimes WHERE environment_id=ANY($1) AND successor_of IS NULL ORDER BY id FOR UPDATE")
        .bind(&environments).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let mut recipients = notifications::operation_audience(conn, principal, &environments, &[])
        .await
        .map_err(ApiError::engine)?;
    notifications::extend_operation_audience(
        conn,
        principal,
        &mut recipients,
        &[(object, Permission::MountManage)],
    )
    .await
    .map_err(ApiError::engine)?;
    recipients.retain(|person| person.user != principal.user());
    let ids = recipients
        .iter()
        .map(|person| person.user)
        .collect::<Vec<_>>();
    let named =
        orch_db::queries::notifications::operations::visible_names(conn, principal.user(), &ids)
            .await?;
    let mut visible_people = BTreeSet::new();
    for id in named {
        if let Some(book) = book.as_deref_mut()
            && !book.account_name(conn, id).await?
        {
            continue;
        }
        visible_people.insert(id);
    }
    let place = root
        .home_id
        .map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let notify = notifications::operation_default(conn, place, !recipients.is_empty())
        .await
        .map_err(ApiError::engine)?;
    let purge_default = orch_settings::get::<String>(
        conn,
        place,
        orch_settings_contract::registry::keys::MOUNT_DELETE_DATA_FATE,
    )
    .await
    .map_err(ApiError::internal)?
    .as_deref()
        == Some("purge");
    let mut prepared = Prepared {
        snapshot,
        root,
        locations,
        rows: Vec::new(),
        targets: HashMap::new(),
        effects,
        environments,
        recipients,
        visible_people,
        notify,
        purge_default,
    };
    let mut data = kept;
    for location in &prepared.locations {
        if data.iter().any(|row| {
            row.machine_id == location.machine_id
                && row.kind == location.kind
                && row.source == location.source
        }) {
            continue;
        }
        data.push(rows::KeptRow {
            id: location.membership_id,
            mount_id: prepared.root.id,
            machine_id: location.machine_id,
            kind: location.kind.clone(),
            source: location.source.clone(),
            storage: location.storage.clone(),
            data_kind: location.data_kind.clone(),
            file_count: None,
            total_bytes: location.last_bytes,
            job_id: None,
            kept_by: principal.user(),
            kept_at: location.created_at,
        });
    }
    for data in data {
        let key = format!("mount:{}", data.id);
        if let Some(book) = book.as_deref_mut() {
            book.keep_object(
                conn,
                key.clone(),
                object,
                json!([
                    data.id,
                    data.machine_id,
                    data.kind,
                    data.source,
                    data.storage
                ]),
            )
            .await?;
        }
        let physical = matches!(
            orch_authz::resources::may_add_location(
                conn,
                principal,
                &LocationAsk {
                    mount: prepared.root.id,
                    machine: data.machine_id,
                    kind: if data.kind == "volume" {
                        LocationKind::Volume
                    } else {
                        LocationKind::Path
                    },
                    storage: data.storage.clone(),
                },
                &Default::default()
            )
            .await
            .map_err(ApiError::internal)?,
            Locating::Allowed
        );
        let held = !rows::running_uses(conn, prepared.root.id, data.machine_id)
            .await?
            .is_empty();
        let maker = rows::data_driver(conn, data.machine_id).await?.is_some();
        let mut purge = physical && maker && data.kind == "volume" && !held;
        if purge && let Some(book) = book.as_deref_mut() {
            purge = book.permits_destination(data.machine_id)?;
            if purge {
                book.destination(conn, None, data.machine_id).await?;
            }
        }
        let mut row = record(
            key.clone(),
            api::StorageKind::MountData,
            prepared.root.name.clone(),
            Time::new(ctx.now),
        );
        row.resource = Some(object);
        row.since = Some(Time::new(data.kept_at));
        row.why = api::KeptWhy::Restore;
        row.availability = api::StorageAvailability::Ready;
        row.can_clean = purge;
        row.bytes = physical.then_some(data.total_bytes).flatten();
        if let Some((machine, host)) =
            crate::routes::resources::mounts::seen(ctx, conn, principal, data.machine_id).await?
        {
            if let Some(book) = book.as_deref_mut() {
                book.machine_name(data.machine_id)?;
            }
            row.machine = Some(machine);
            row.host = host;
        }
        row.blocker = if data.kind == "path" {
            Some("borrowed_path".into())
        } else if held {
            Some("in_use".into())
        } else if !purge {
            Some("not_permitted".into())
        } else {
            None
        };
        if purge {
            prepared.targets.insert(
                key,
                Target::MountData {
                    mount: prepared.root.id,
                    kept: data.id,
                },
            );
        }
        prepared.rows.push(row);
    }
    for lost in lost {
        let key = format!("mount:{}", lost.id);
        if let Some(book) = book.as_deref_mut() {
            book.keep_object(conn, key.clone(), object, json!([lost.id, lost.lost_at]))
                .await?;
        }
        let mut row = record(
            key,
            api::StorageKind::MountData,
            prepared.root.name.clone(),
            Time::new(ctx.now),
        );
        row.resource = Some(object);
        row.since = Some(Time::new(lost.lost_at));
        row.bytes = lost.total_bytes;
        row.why = api::KeptWhy::Restore;
        row.availability = api::StorageAvailability::Unknown;
        row.blocker = Some("node_away".into());
        prepared.rows.push(row);
    }
    Ok(prepared)
}

impl Prepared {
    pub(super) fn records(&self) -> PreparedRecords {
        let rows = self.rows.clone();
        PreparedRecords {
            targets: self.targets.clone(),
            render: Box::new(move || Ok(("me".into(), rows.clone(), Vec::new()))),
        }
    }

    pub(super) async fn plan(&self, conn: &mut Conn, plan: &mut api::CleanupPlan) -> ApiResult<()> {
        plan.effects = Some(self.effects.clone());
        plan.people = refusals::user_refs(
            conn,
            &self.visible_people.iter().copied().collect::<Vec<_>>(),
        )
        .await?
        .into_values()
        .collect();
        plan.people.sort_by_key(|person| person.id);
        plan.other_people = i64::try_from(self.recipients.len().saturating_sub(plan.people.len()))
            .map_err(ApiError::internal)?;
        plan.notify = self.notify;
        if self.purge_default {
            for item in &mut plan.items {
                if item.choices.contains(&CleanupChoice::Purge) {
                    item.selected = CleanupChoice::Purge;
                }
            }
        }
        Ok(())
    }

    pub(super) fn check(&self, saved: &Json) -> ApiResult<()> {
        let original: Snapshot =
            serde_json::from_value(saved.clone()).map_err(ApiError::internal)?;
        if original != self.snapshot {
            return Err(ApiError::new(
                ErrorCode::PreflightExpired,
                "the mount or its data changed; read the deletion plan again",
            ));
        }
        Ok(())
    }

    pub(super) async fn retain(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        _job: Id,
        work: &mut [Work],
        at: chrono::DateTime<chrono::Utc>,
    ) -> ApiResult<()> {
        for location in &self.locations {
            if location.kind != "volume" {
                continue;
            }
            rows::keep(
                conn,
                &rows::KeptRow {
                    id: location.membership_id,
                    mount_id: self.root.id,
                    machine_id: location.machine_id,
                    kind: location.kind.clone(),
                    source: location.source.clone(),
                    storage: location.storage.clone(),
                    data_kind: location.data_kind.clone(),
                    file_count: None,
                    total_bytes: location.last_bytes,
                    job_id: None,
                    kept_by: principal.user(),
                    kept_at: at,
                },
            )
            .await?;
            let kept: Id = sqlx::query_scalar("SELECT id FROM mount_kept_data WHERE mount_id=$1 AND machine_id=$2 AND kind=$3 AND source=$4 AND lost_at IS NULL")
                .bind(self.root.id).bind(location.machine_id).bind(&location.kind).bind(&location.source)
                .fetch_one(&mut *conn).await.map_err(DbError::from)?;
            for item in work.iter_mut() {
                if item.id == format!("mount:{}", location.membership_id) && item.target.is_some() {
                    item.target = Some(Target::MountData {
                        mount: self.root.id,
                        kept,
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) async fn remove(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
        job: Id,
        notify: bool,
    ) -> ApiResult<()> {
        let key = self.root.id.to_string();
        sqlx::query("UPDATE settings SET value=value-$1,revision=revision+1,updated_at=$2 WHERE key='env.mounts' AND value ? $1")
            .bind(&key).bind(ctx.now).execute(&mut *conn).await.map_err(DbError::from)?;
        sqlx::query("UPDATE environments SET config=jsonb_set(config,'{env.mounts}',(config->'env.mounts')-$1),revision=revision+1,updated_at=$2 WHERE config->'env.mounts' ? $1")
            .bind(&key).bind(ctx.now).execute(&mut *conn).await.map_err(DbError::from)?;
        sqlx::query("UPDATE runtimes SET state_detail='an update is available: mount attachments changed',updated_at=$2 WHERE environment_id=ANY($1) AND successor_of IS NULL AND state NOT IN ('retired','removed')")
            .bind(&self.environments).bind(ctx.now).execute(&mut *conn).await.map_err(DbError::from)?;
        sqlx::query("DELETE FROM mount_locations WHERE mount_id=$1")
            .bind(self.root.id)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        rows::delete(conn, self.root.id, ctx.now).await?;
        orch_db::queries::access::bump_epoch(conn).await?;
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(principal.user()),
            orch_db::audit::action!(orch_access_contract::audit::AuditAction::MountDeleted)
                .as_str(),
            "mount",
            Some(self.root.id),
            "Deleted a mount",
        );
        entry.history = true;
        entry.node_id = self.root.home_id;
        entry.details = json!({"mount":self.root.id,"name":self.root.name,"job":job,"items":self.rows.iter().map(|row| &row.id).collect::<Vec<_>>()});
        orch_db::audit::append(conn, &entry, &orch_db::audit::current_origin(), ctx.now).await?;
        let operation = NotificationOperation {
            actor: principal.user(),
            id: job,
            kind: "mount_delete".into(),
            name: self.root.name.clone(),
            notify,
            path: "/resources/mounts".into(),
            subject: Some(("mount".into(), self.root.id)),
            recipients: self.recipients.clone(),
        };
        notifications::attach_operation(conn, &[job], &operation, ctx.now)
            .await
            .map_err(ApiError::engine)?;
        notifications::notify_operation(conn, &operation, "running", ctx.now)
            .await
            .map_err(ApiError::engine)
    }
}
