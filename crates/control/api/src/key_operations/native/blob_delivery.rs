//! Original native command sources at internal byte publication, without an HTTP Ctx.
//! The domain owns its exact hash-specific scope/ordinary verifier and immutable witness.
//! This bridge owns current canonical command/Machine association and transport cuts only.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::state::App;
use axum::response::Response;
use futures::future::BoxFuture;
use orch_api_contract::common::ErrorCode;
use orch_authz::Caller;
use orch_db::queries::{link, resources, transaction_guard};
use orch_db::{Conn, DbError};
use orch_engine::commands::CommandAuthority;
use orch_types::{Hash, Id, Time};
use std::sync::Arc;

/// Actual current native callers and raw source ends, returned by the source verifier.
/// An empty set is only an explicitly verified native/system purpose, never error fallback.
pub(crate) type Callers = Vec<(Option<Caller>, Option<Time>)>;

/// One hash-specific native source provider, selected by the actual private authority.
/// Both methods are effect-free. They must validate the original operation, every retained
/// key/account/raw Session door, actual declared hash and fixed native source association,
/// even when the command was already Accepted. They never authorize bytes from another
/// field/custodian or reuse the command's accepted-cleanup exemption as source Read.
pub(crate) trait Source: Send + Sync + 'static {
    type Input: Send + Sync + 'static;
    /// Actual source opened once before store IO, not reusable admission or a new selector.
    type Opened: Send + Sync + 'static;

    fn prepare<'a>(
        app: &'a App,
        conn: &'a mut Conn,
        command: &'a link::CommandRow,
        authority: Option<&'a CommandAuthority>,
        hash: &'a Hash,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<Option<(Self::Opened, Callers)>>>;

    /// Refresh that same original native witness after awaited storage/backpressure.
    /// None means closed; malformed/present evidence must never become native old work.
    fn refresh<'a>(
        app: &'a App,
        conn: &'a mut Conn,
        command: &'a link::CommandRow,
        authority: Option<&'a CommandAuthority>,
        hash: &'a Hash,
        input: &'a Self::Input,
        opened: &'a Self::Opened,
    ) -> BoxFuture<'a, ApiResult<Option<Callers>>>;
}

struct Original {
    command: link::CommandRow,
    authority: Option<orch_types::Json>,
}
impl Original {
    fn matches(&self, current: &link::CommandWithAuthority) -> bool {
        let before = &self.command;
        let now = &current.command;
        before.id == now.id
            && before.machine_id == now.machine_id
            && before.kind == now.kind
            && before.body == now.body
            && before.serial_key() == now.serial_key()
            && before.created_at == now.created_at
            && before.deadline_at == now.deadline_at
            && self.authority == current.authority
    }
}

struct Inner<F: Source> {
    app: App,
    original: Original,
    hash: Hash,
    input: F::Input,
    opened: F::Opened,
}

/// Immutable actual command/private authority/hash/native source across byte waits.
/// No transaction, connection, caller admission or growing frame history is retained.
#[must_use = "check the native source after awaits and at its metadata/body boundary"]
pub(crate) struct Delivery<F: Source>(Arc<Inner<F>>);
impl<F: Source> Clone for Delivery<F> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the original native byte source is unavailable",
    )
}
fn decode(row: &link::CommandWithAuthority) -> ApiResult<Option<CommandAuthority>> {
    row.authority
        .as_ref()
        .map(|value| serde_json::from_value(value.clone()).map_err(|_| refused()))
        .transpose()
}
fn live(app: &App, callers: &Callers) -> ApiResult<()> {
    if callers.iter().all(|(caller, until)| {
        caller
            .as_ref()
            .is_none_or(|caller| caller.key_deadline_allows(app.clock.as_ref()))
            && until.is_none_or(|until| app.clock.now() < until.get())
    }) {
        Ok(())
    } else {
        Err(refused())
    }
}

async fn current(
    conn: &mut Conn,
    machine: Id,
    command: Id,
) -> ApiResult<link::CommandWithAuthority> {
    let machine = resources::machine(conn, machine)
        .await?
        .filter(resources::MachineRow::is_active)
        .ok_or_else(refused)?;
    let row = link::command_with_authority(conn, command)
        .await?
        .ok_or_else(refused)?;
    if row.command.machine_id != machine.id {
        return Err(refused());
    }
    if row.command.is_over() {
        return Err(ApiError::new(ErrorCode::Gone, "that command is over"));
    }
    // Match the native internal listener's existing command state rule, including Accepted.
    Ok(row)
}

/// Recheck after the native source's awaits, keeping Machine before command lock order.
/// Source/key restoration has already completed on this connection. These short locks
/// close revocation/cancellation/replacement races through commit, never through IO.
async fn seal(conn: &mut Conn, original: &Original) -> ApiResult<()> {
    let machine: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM machines WHERE id=$1 AND status='active' AND deleted_at IS NULL FOR SHARE",
    )
    .bind(original.command.machine_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    if machine.is_none() {
        return Err(refused());
    }
    let current = link::lock_command_with_authority(conn, original.command.id)
        .await?
        .ok_or_else(refused)?;
    if !original.matches(&current) {
        return Err(refused());
    }
    if current.command.is_over() {
        return Err(ApiError::new(ErrorCode::Gone, "that command is over"));
    }
    Ok(())
}

/// Invoke before opening the store. Machine is the actual certificate-authenticated
/// request identity; the native listener still checks protocol kind/list/range rules.
/// The domain's Input comes from its private native source, never a caller-only lease.
pub(crate) async fn prepare<F: Source>(
    app: &App,
    machine: Id,
    command: Id,
    hash: Hash,
    input: F::Input,
) -> ApiResult<Delivery<F>> {
    if command.is_nil() || machine.is_nil() {
        return Err(refused());
    }
    let mut tx = app.db.begin().await?;
    transaction_guard::lock_action(&mut tx).await?;
    let row = current(&mut tx, machine, command).await?;
    let authority = decode(&row)?;
    let (opened, callers) = F::prepare(
        app,
        &mut tx,
        &row.command,
        authority.as_ref(),
        &hash,
        &input,
    )
    .await?
    .ok_or_else(refused)?;
    let original = Original {
        command: row.command,
        authority: row.authority,
    };
    seal(&mut tx, &original).await?;
    tx.commit().await.map_err(DbError::from)?;
    live(app, &callers)?;
    Ok(Delivery(Arc::new(Inner {
        app: app.clone(),
        original,
        hash,
        input,
        opened,
    })))
}

impl<F: Source> Delivery<F> {
    /// Same frozen command/authority/hash and native source after a later await.
    /// The native original/current checker runs even for already Accepted commands.
    pub(crate) async fn check(&self) -> ApiResult<()> {
        let frame = &self.0;
        let mut tx = frame.app.db.begin().await?;
        transaction_guard::lock_action(&mut tx).await?;
        let row = current(
            &mut tx,
            frame.original.command.machine_id,
            frame.original.command.id,
        )
        .await?;
        if !frame.original.matches(&row) {
            return Err(refused());
        }
        let authority = decode(&row)?;
        let callers = F::refresh(
            &frame.app,
            &mut tx,
            &row.command,
            authority.as_ref(),
            &frame.hash,
            &frame.input,
            &frame.opened,
        )
        .await?
        .ok_or_else(refused)?;
        seal(&mut tx, &frame.original).await?;
        tx.commit().await.map_err(DbError::from)?;
        live(&frame.app, &callers)
    }

    /// Invoke after awaited store read/touch, including metadata-only 416/HEAD results.
    /// Preserve the original response parts, trailers, errors and transfer-count owner.
    /// Every successful upstream frame is checked after it becomes ready and before yield;
    /// no store reopen, source selection, renderer or body IO occurs in snapshot retries.
    pub(crate) async fn response(&self, response: Response) -> ApiResult<Response> {
        let original = self.clone();
        crate::response_body::checked(response, move || {
            let frame = original.clone();
            async move { frame.check().await }
        })
        .await
    }
}
