//! Accepted environment lifecycle execution in the existing lifecycle command family.
use super::*;
use crate::{access::ResourceAccess, operation_scope::http_origin::HttpOrigin};
pub use jobs::environment_move::preparation::execution::{Evidence, Selection};
const FIELD: &str = "environment_lifecycle";
const VERSION: &str = "environment_lifecycle_version";
const REQUIRED: &str = "environment_lifecycle_required";

fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}
fn raw_error(error: crate::operation_scope::continuing::Error) -> EngineError {
    match error {
        crate::operation_scope::continuing::Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retained {
    job: Id,
    kind: String,
    actor: Id,
    environment: Option<Id>,
    native: Box<Evidence>,
    accepted: HttpOrigin,
}
pub fn present(row: &rows::JobRow) -> bool {
    [FIELD, VERSION, REQUIRED]
        .iter()
        .any(|field| row.plan.get(*field).is_some())
}
fn read(row: &rows::JobRow) -> Result<Option<Retained>, EngineError> {
    if !present(row) {
        return Ok(None);
    }
    if row.plan.get(VERSION) != Some(&serde_json::json!(1))
        || row.plan.get(REQUIRED) != Some(&serde_json::json!(true))
    {
        return Err(refused());
    }
    let retained: Retained =
        serde_json::from_value(row.plan.get(FIELD).cloned().ok_or_else(refused)?)
            .map_err(|_| refused())?;
    if retained.job != row.id
        || retained.kind != row.kind
        || row.created_by != Some(retained.actor)
        || retained.actor != retained.accepted.origin().user()
        || retained.environment != row.environment_id
        || !matches!(
            row.kind.as_str(),
            rows::kind::ENVIRONMENT_PROVISION
                | rows::kind::ENVIRONMENT_RECREATE
                | "environment_move"
                | jobs::fork::KIND
                | rows::kind::PROVIDER_PROVISION
                | rows::kind::PROVIDER_RECREATE
        )
        || !retained
            .native
            .permits_operation(retained.accepted.origin().operation())
        || !retained.native.matches_row(row).unwrap_or(false)
        || !retained.accepted.same_identity(retained.native.origin())
    {
        return Err(refused());
    }
    Ok(Some(retained))
}

/// The exact original/current HTTP/key tail, after already-authorized native SQL writes.
/// This does not re-admit a completed business mutation or refresh its physical operands.
pub(crate) async fn check_deadline(
    conn: &mut Conn,
    row: &rows::JobRow,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    let source = read(row)?.ok_or_else(refused)?;
    let raw = source
        .accepted
        .current(conn, clock)
        .await
        .map_err(raw_error)?;
    raw.check(clock).map_err(raw_error)?;
    if source
        .accepted
        .until()
        .is_some_and(|until| clock.time() >= until)
        || !raw.caller().key_deadline_allows(clock)
    {
        return Err(refused());
    }
    Ok(())
}

pub(crate) fn owns_recipe(row: &rows::JobRow) -> Result<bool, EngineError> {
    Ok(read(row)?.is_some())
}
pub(crate) async fn owns_provider_candidate(
    conn: &mut Conn,
    row: &rows::JobRow,
) -> Result<bool, EngineError> {
    read(row)?
        .ok_or_else(refused)?
        .native
        .owns_provider_candidate(conn, row)
        .await
}
pub(crate) fn preserves(
    previous: &rows::JobRow,
    current: &rows::JobRow,
) -> Result<(), EngineError> {
    match (read(previous)?, read(current)?) {
        (None, None) => Ok(()),
        (Some(a), Some(b))
            if a.job == b.job
                && a.kind == b.kind
                && a.actor == b.actor
                && a.environment == b.environment
                && a.native == b.native
                && a.accepted.same_identity(&b.accepted) =>
        {
            Ok(())
        }
        _ => Err(refused()),
    }
}
pub(crate) async fn lock_source(conn: &mut Conn, row: &rows::JobRow) -> Result<(), EngineError> {
    if let Some(source) = read(row)? {
        source.accepted.lock_identity(conn).await?;
        source.native.lock_source(conn, row).await?;
    }
    Ok(())
}
pub(crate) fn deadline(row: &rows::JobRow) -> Result<Option<Time>, EngineError> {
    Ok(read(row)?.ok_or_else(refused)?.accepted.until())
}
pub struct Checkpoint {
    pub principal: Principal,
    pub until: Option<Time>,
}
/// Current original-source principal for native consumers with or without an existing transaction.
/// The native scope and raw identity are preserved; no account-only Session is constructed.
pub async fn principal(
    conn: &mut Conn,
    row: &rows::JobRow,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
    selected_host: Option<Id>,
) -> Result<Checkpoint, EngineError> {
    if orch_db::queries::transaction_guard::in_transaction(conn) {
        return current(conn, row, clock, access, selected_host).await;
    }
    use sqlx::Connection as _;
    let mut tx = conn.begin().await.map_err(orch_db::DbError::from)?;
    let checked = current(&mut tx, row, clock, access, selected_host).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    checked.check(clock)?;
    Ok(checked)
}
/// Provider-only compatibility for native replacement notices and final command checks.
/// It uses the same retained source, locks, current identity and minimum persistence.
pub(crate) async fn provider_current(
    conn: &mut Conn,
    row: &rows::JobRow,
    clock: &dyn Clock,
) -> Result<Checkpoint, EngineError> {
    if !read(row)?.is_some_and(|source| source.native.is_provider()) {
        return Err(refused());
    }
    if orch_db::queries::transaction_guard::in_transaction(conn) {
        return current_selected(conn, row, clock, Selection::Current).await;
    }
    use sqlx::Connection as _;
    let mut tx = conn.begin().await.map_err(orch_db::DbError::from)?;
    let checked = current_selected(&mut tx, row, clock, Selection::Current).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    checked.check(clock)?;
    Ok(checked)
}
impl Checkpoint {
    pub fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        if !self.principal.caller().key_deadline_allows(clock)
            || self.until.is_some_and(|until| clock.time() >= until)
        {
            return Err(refused());
        }
        Ok(())
    }
}
async fn checked(
    conn: &mut Conn,
    row: &rows::JobRow,
    source: &Retained,
    clock: &dyn Clock,
    selection: Selection,
) -> Result<(Checkpoint, HttpOrigin), EngineError> {
    if !row.is_open() || row.cancel_requested() || jobs::runner::abort_reason(row)?.is_some() {
        return Err(refused());
    }
    let accepted = source
        .accepted
        .intersect(&source.native.current_origin(conn, clock).await?)
        .map_err(raw_error)?;
    let raw = accepted.current(conn, clock).await.map_err(raw_error)?;
    let principal = accepted
        .scope(source.native.current_principal(conn, row, clock, selection))
        .await?;
    if principal.user() != source.actor
        || principal.caller().admitted_operation() != Some(source.accepted.origin().operation())
        || principal.caller().admitted_required_roles().is_none()
    {
        return Err(refused());
    }
    let principal = principal
        .within_source(raw.caller())
        .map_err(|_| refused())?;
    let accepted = accepted
        .accepted(principal.caller(), raw.until())
        .map_err(raw_error)?;
    let checkpoint = Checkpoint {
        principal,
        until: accepted.until(),
    };
    checkpoint.check(clock)?;
    Ok((checkpoint, accepted))
}
pub async fn current(
    conn: &mut Conn,
    previous: &rows::JobRow,
    clock: &dyn Clock,
    _access: &dyn ResourceAccess,
    selected_host: Option<Id>,
) -> Result<Checkpoint, EngineError> {
    current_selected(
        conn,
        previous,
        clock,
        selected_host.map_or(Selection::Current, Selection::Host),
    )
    .await
}
pub(crate) async fn current_wake(
    conn: &mut Conn,
    previous: &rows::JobRow,
    environment: Id,
    clock: &dyn Clock,
    _access: &dyn ResourceAccess,
) -> Result<Checkpoint, EngineError> {
    current_selected(conn, previous, clock, Selection::Wake(environment)).await
}
async fn current_selected(
    conn: &mut Conn,
    previous: &rows::JobRow,
    clock: &dyn Clock,
    selection: Selection,
) -> Result<Checkpoint, EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(refused());
    }
    lock_source(conn, previous).await?;
    let row = rows::lock(conn, previous.id).await?.ok_or_else(refused)?;
    preserves(previous, &row)?;
    let mut source = read(&row)?.ok_or_else(refused)?;
    let previous = read(previous)?.ok_or_else(refused)?;
    source.accepted = source
        .accepted
        .intersect(&previous.accepted)
        .map_err(raw_error)?;
    let (checkpoint, accepted) = checked(conn, &row, &source, clock, selection).await?;
    source.accepted = accepted;
    rows::merge_plan(
        conn,
        row.id,
        &serde_json::json!({FIELD:source}),
        clock.now(),
    )
    .await?;
    checkpoint.check(clock)?;
    Ok(checkpoint)
}
/// Attach only to the actual newly queued native job, in its queue transaction.
pub async fn retain(
    conn: &mut Conn,
    job: Id,
    native: Evidence,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<(), EngineError> {
    retain_within(conn, job, native, None, clock, access).await
}
async fn retain_within(
    conn: &mut Conn,
    job: Id,
    native: Evidence,
    earlier: Option<&HttpOrigin>,
    clock: &dyn Clock,
    _access: &dyn ResourceAccess,
) -> Result<(), EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(refused());
    }
    let accepted = match earlier {
        Some(earlier) => native.origin().intersect(earlier).map_err(raw_error)?,
        None => native.origin().clone(),
    };
    accepted.lock_identity(conn).await?;
    let row = rows::job(conn, job).await?.ok_or_else(refused)?;
    native.lock_source(conn, &row).await?;
    let row = rows::lock(conn, job).await?.ok_or_else(refused)?;
    let inserted_here: bool = sqlx::query_scalar("SELECT xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid FROM jobs WHERE id=$1").bind(job).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if !inserted_here
        || row.state != "queued"
        || row.created_by != Some(accepted.origin().user())
        || row.plan.get("creator_provision").is_some()
    {
        return Err(refused());
    }
    let mut source = Retained {
        job,
        kind: row.kind.clone(),
        actor: accepted.origin().user(),
        environment: row.environment_id,
        native: Box::new(native),
        accepted,
    };
    let proposed = serde_json::json!({ FIELD:source, VERSION:1, REQUIRED:true });
    let mut candidate = row.clone();
    let map = candidate.plan.as_object_mut().ok_or_else(refused)?;
    for (field, value) in proposed.as_object().ok_or_else(refused)? {
        map.insert(field.clone(), value.clone());
    }
    read(&candidate)?.ok_or_else(refused)?;
    if let Some(old) = read(&row)? {
        if old.native != source.native || !old.accepted.same_identity(&source.accepted) {
            return Err(refused());
        }
        source.accepted = source
            .accepted
            .intersect(&old.accepted)
            .map_err(raw_error)?;
    }
    let (checkpoint, accepted) = checked(conn, &row, &source, clock, Selection::Current).await?;
    source.accepted = accepted;
    rows::merge_plan(
        conn,
        job,
        &serde_json::json!({FIELD:source,VERSION:1,REQUIRED:true}),
        clock.now(),
    )
    .await?;
    checkpoint.check(clock)
}
/// The native fork/maker keeps the original lifecycle operation on its actual new child.
pub async fn retain_child(
    conn: &mut Conn,
    parent: &rows::JobRow,
    child: Id,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<(), EngineError> {
    let checkpoint = current(conn, parent, clock, access, None).await?;
    let parent = rows::job(conn, parent.id).await?.ok_or_else(refused)?;
    let source = read(&parent)?.ok_or_else(refused)?;
    let child_row = rows::job(conn, child).await?.ok_or_else(refused)?;
    let native = source
        .accepted
        .scope(
            source
                .native
                .child(conn, &parent, &child_row, clock, access),
        )
        .await?;
    if !source.accepted.same_identity(native.origin()) {
        return Err(refused());
    }
    retain_within(conn, child, native, Some(&source.accepted), clock, access).await?;
    checkpoint.check(clock)
}
pub(crate) async fn cleanup_matches(
    conn: &mut Conn,
    row: &rows::JobRow,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    let source = read(row)?.ok_or_else(refused)?;
    source
        .native
        .cleanup_matches(conn, row, machine, key, kind, body)
        .await
}
