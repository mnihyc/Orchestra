//! The original native environment deletion after its bindings have been settled.
use crate::{Engine, EngineError, archive::RemovalScheduleError};
use chrono::{DateTime, Utc};
use orch_db::{
    Conn,
    queries::environments::{retained, routes::EnvironmentFull},
};
use orch_types::{Id, Time};

/// The native disposal result; presentation and its original audit stay with the caller.
pub enum Disposition {
    Lost,
    Connected {
        purge_at: Option<DateTime<Utc>>,
    },
    Scheduled {
        at: DateTime<Utc>,
        accepted: bool,
        removal: Box<retained::Removal>,
    },
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Native(#[from] EngineError),
    #[error(transparent)]
    Retention(EngineError),
    #[error(transparent)]
    Schedule(#[from] RemovalScheduleError),
    #[error("the accepted environment removal disappeared")]
    Gone,
}
impl From<orch_db::DbError> for Error {
    fn from(error: orch_db::DbError) -> Self {
        Self::Native(error.into())
    }
}
/// The same lost-registration, connected registration and retained managed removal branches.
pub async fn apply(
    engine: &Engine,
    conn: &mut Conn,
    row: &EnvironmentFull,
    by: Id,
    at: Option<Time>,
    now: Time,
) -> Result<Disposition, Error> {
    if row.status == "lost" {
        engine
            .remove_lost_environment_in(conn, row.id, by, now.get())
            .await?;
        return Ok(Disposition::Lost);
    }
    if orch_db::queries::environment_facts::of(conn, row.id)
        .await?
        .is_some_and(|facts| facts.attaches())
    {
        engine
            .schedule_environment_removal(conn, row.id, Some(by), now.get(), now.get())
            .await
            .map_err(Error::Retention)?;
        let promise = retained::removal(conn, row.id).await?.ok_or(Error::Gone)?;
        retained::begin_remove(conn, row.id, promise.removal_revision).await?;
        retained::keep_removed(conn, row.id, promise.removal_revision, now.get()).await?;
        let purge_at = retained::removal(conn, row.id)
            .await?
            .and_then(|promise| promise.volumes_purge_at);
        Ok(Disposition::Connected { purge_at })
    } else {
        let at = row
            .remove_at
            .unwrap_or_else(|| at.map_or(now.get(), Time::get));
        let accepted = engine
            .schedule_removal_in(conn, row.id, at, by, now)
            .await?;
        let removal = retained::removal(conn, row.id).await?.ok_or(Error::Gone)?;
        Ok(Disposition::Scheduled {
            at: row.remove_at.unwrap_or(at),
            accepted,
            removal: Box::new(removal),
        })
    }
}

/// An exact reviewed original, never a reusable admission or a lease.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    source: Id,
    stamp: orch_types::Hash,
}
impl Review {
    #[must_use]
    pub fn source(&self) -> Id {
        self.source
    }
    #[must_use]
    pub fn stamp(&self) -> orch_types::Hash {
        self.stamp
    }
}
/// Select the same physical/binding/deletion consequences that the native removal will consume.
pub async fn review(conn: &mut Conn, row: &EnvironmentFull) -> Result<Review, Error> {
    if row.status != "lost"
        && !orch_db::queries::environment_facts::of(conn, row.id)
            .await?
            .is_some_and(|facts| facts.attaches())
    {
        crate::archive::check_environment_removal(conn, row.id).await?;
    }
    let mut bindings = orch_db::queries::lifecycle::bindings(conn, row.id).await?;
    bindings.sort_unstable();
    let work:serde_json::Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(jsonb_build_array(node_id,state,current_segment_id,current_turn_id,environment_id) ORDER BY node_id),'[]'::jsonb) FROM threads WHERE node_id=ANY($1)").bind(&bindings).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let stamp = crate::jobs::command_identity::hash(
        &serde_json::json!({"source":row.id,"revision":row.revision,"home":row.home_node_id,"org":row.org_id,"placement":row.placement,"machine":row.machine_id,"template":row.template_id,"generation":row.generation,"root":row.workspace_root,"remove_at":row.remove_at,"bindings":bindings,"work":work}),
    )?;
    Ok(Review {
        source: row.id,
        stamp,
    })
}
/// Only the copy's actual source has the ending role; a pending target never supplies it.
pub(crate) fn moving_review(
    row: &orch_db::queries::jobs::JobRow,
) -> Result<Option<Review>, EngineError> {
    let operation = row
        .plan
        .pointer("/creator_provision/origin/operation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !orch_api_contract::environment_sources::duplicating::is_move(operation) {
        if row.plan.get("environment_move_out").is_some() {
            return Err(EngineError::unreadable(
                "ordinary creation cannot borrow a moving source",
            ));
        }
        return Ok(None);
    }
    let review: Review = serde_json::from_value(row.plan["environment_move_out"].clone())?;
    let files: crate::initial_files::Plan =
        serde_json::from_value(row.plan["initial_files"].clone())?;
    if files.source_thread().is_some()
        || files.source_environment() != review.source
        || Some(review.source) == row.environment_id
    {
        return Err(EngineError::unreadable(
            "the moving source differs from its actual workspace copy",
        ));
    }
    Ok(Some(review))
}
/// Same actual owner, Manage, byte-source association and source policy at each native wait.
pub(crate) async fn require_move(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    row: &orch_db::queries::jobs::JobRow,
) -> Result<(), EngineError> {
    let Some(accepted) = moving_review(row)? else {
        return Ok(());
    };
    let source = orch_db::queries::environments::routes::environments(conn, &[accepted.source])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("the moving source disappeared"))?;
    if review(conn, &source)
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?
        != accepted
    {
        return Err(EngineError::unreadable(
            "the reviewed environment deletion changed",
        ));
    }
    let subject = crate::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
        kind: orch_access_contract::permissions::ObjectKind::Environment,
        id: source.id,
    });
    if !crate::copy_out::owns(conn, principal, subject)
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?
        || !orch_authz::resources::check(
            conn,
            principal,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: source.id,
            },
            orch_access_contract::permissions::Permission::EnvironmentManage,
            orch_authz::Policy::default(),
        )
        .await?
        .is_allowed()
    {
        return Err(EngineError::unreadable(
            "only a current owner may end this environment",
        ));
    }
    let home: Id = serde_json::from_value(
        row.plan
            .pointer("/creator_provision/home")
            .cloned()
            .ok_or_else(|| EngineError::unreadable("the moving destination disappeared"))?,
    )?;
    if crate::copy_out::authorize(conn, principal, subject, Some(home))
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?
        .is_none()
    {
        return Err(EngineError::unreadable(
            "the environment no longer moves to another organization",
        ));
    }
    Ok(())
}
/// Native success commit consumes original Delete once, after exact initial files are delivered.
pub(crate) async fn finish(
    engine: &Engine,
    conn: &mut Conn,
    row: &orch_db::queries::jobs::JobRow,
    principal: &orch_authz::Principal,
    clock: &dyn orch_types::Clock,
) -> Result<(), EngineError> {
    let Some(accepted) = moving_review(row)? else {
        return Ok(());
    };
    require_move(conn, principal, row).await?;
    let raw: crate::operation_scope::http_origin::HttpOrigin =
        serde_json::from_value(row.plan["duplicate_http_origin"].clone())?;
    if raw.origin().user() != principal.user()
        || principal.caller().admitted_operation() != Some(raw.origin().operation())
        || raw.until().is_some_and(|end| clock.time() >= end)
        || !principal.caller().key_deadline_allows(clock)
    {
        return Err(EngineError::unreadable(
            "the environment move lifetime ended",
        ));
    }
    let source = orch_db::queries::environments::routes::environments(conn, &[accepted.source])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("the moving source disappeared"))?;
    let target = row
        .environment_id
        .ok_or_else(|| EngineError::unreadable("the native copy target disappeared"))?;
    let home: Id = serde_json::from_value(row.plan["creator_provision"]["home"].clone())?;
    let boundary = crate::copy_out::authorize(
        conn,
        principal,
        crate::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: source.id,
        }),
        Some(home),
    )
    .await
    .map_err(|error| EngineError::unreadable(error.to_string()))?
    .ok_or_else(|| {
        EngineError::unreadable("the source no longer leaves its original organization")
    })?;
    let origin = orch_db::audit::Origin {
        as_admin: raw.as_admin().then_some(principal.user()),
        ..Default::default()
    };
    crate::copy_out::record(
        conn,
        boundary,
        orch_db::audit::Actor::User(principal.user()),
        &origin,
        target,
        clock.now(),
    )
    .await?;
    let settled = engine
        .remove_bindings_in(conn, source.id, principal.user(), clock.time())
        .await?;
    let ended = apply(engine, conn, &source, principal.user(), None, clock.time())
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?;
    let (action, details) = match ended {
        Disposition::Lost => (
            orch_access_contract::audit::AuditAction::EnvironmentDeleted,
            serde_json::json!({"environment":source.id,"threads":settled.threads,"turns":settled.turns}),
        ),
        Disposition::Connected { purge_at } => (
            orch_access_contract::audit::AuditAction::EnvironmentDeleted,
            serde_json::json!({"environment":source.id,"name":source.name,"purge_at":purge_at}),
        ),
        Disposition::Scheduled { at, removal, .. } => (
            orch_access_contract::audit::AuditAction::EnvironmentRemovalScheduled,
            serde_json::json!({"environment":source.id,"at":at,"removal_revision":removal.removal_revision,"keep_days":removal.removed_keep_days,"threads":settled.threads,"turns":settled.turns}),
        ),
    };
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(principal.user()),
        action.as_str(),
        "environment",
        Some(source.id),
        "ended the original environment after its native copy was prepared",
    );
    entry.history = action.is_history_line();
    entry.node_id = Some(source.home_node_id);
    entry.org_id = Some(source.org_id);
    entry.environment_id = Some(source.id);
    entry.details = details;
    orch_db::audit::append(conn, &entry, &origin, clock.now()).await?;
    orch_authz::epoch::bump(conn).await?;
    if raw.until().is_some_and(|end| clock.time() >= end)
        || !principal.caller().key_deadline_allows(clock)
    {
        return Err(EngineError::unreadable(
            "the environment move lifetime ended",
        ));
    }
    Ok(())
}
