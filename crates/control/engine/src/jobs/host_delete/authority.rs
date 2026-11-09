//! Actual accepted confirmation identity for the parent's own Host control.

use super::*;
use crate::operation_scope::{
    request_origin::{self, Credential},
    sources::{self, Facts},
    targets,
};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    host_delete_sources::{CREATE, Source},
    loss::HostDeleteCreate,
    routes::Endpoint,
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::Conn;
use orch_types::Clock;

/// Presence, including an explicit malformed value, brands a known native source.
pub const SOURCE: &str = "host_delete_authority";
/// A known source never becomes an ordinary caller when one private metadata field is lost.
pub fn present(row: &jobs::JobRow) -> bool {
    [SOURCE, "lifecycle_owner", "lifecycle_commands"]
        .iter()
        .any(|field| row.plan.get(*field).is_some())
}
/// Private real request identity. No session is inferred from an actor id.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_http"
    )]
    http: Option<crate::operation_scope::http_origin::HttpOrigin>,
}
impl Origin {
    /// Capture only the actual admitted confirmation and its real authenticator evidence.
    pub fn capture(
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(CREATE.operation) || !credential.matches(caller) {
            return Err(EngineError::unreadable(
                "the confirmation identity does not match",
            ));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            credential,
            until,
            http: None,
        })
    }
}
fn present_http<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<crate::operation_scope::http_origin::HttpOrigin>, D::Error> {
    crate::operation_scope::http_origin::HttpOrigin::deserialize(d).map(Some)
}
impl Origin {
    /// Retain the actual common HTTP identity and every accepted native preparation minimum.
    pub fn from_http(
        http: crate::operation_scope::http_origin::HttpOrigin,
    ) -> Result<Self, EngineError> {
        if http.origin().operation() != CREATE.operation {
            return Err(EngineError::unreadable(
                "The native confirmation source changed its operation.",
            ));
        }
        Ok(Self {
            origin: http.origin().clone(),
            credential: http.credential().clone(),
            until: http.until(),
            http: Some(http),
        })
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(Caller, Option<orch_types::Time>), EngineError> {
        match &self.http {
            Some(http) => {
                let current = http.current(conn, clock).await.map_err(source_error)?;
                Ok((current.caller().clone(), current.until()))
            }
            None => request_origin::current(
                conn,
                clock,
                &self.origin,
                &self.credential,
                self.until,
                CREATE.operation,
            )
            .await
            .map_err(source_error),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    origin: Origin,
    job: Id,
    host: Id,
    machine: Id,
    recipe: orch_types::Hash,
}
#[derive(Debug, thiserror::Error)]
#[error("the original native lifecycle source is closed: {0}")]
pub(crate) struct Ended(String);
fn denied(error: impl std::fmt::Display) -> EngineError {
    EngineError::BeforeEffect(Box::new(Ended(error.to_string())))
}
fn source_error(error: crate::operation_scope::continuing::Error) -> EngineError {
    match error {
        crate::operation_scope::continuing::Error::Native(error) => error,
        error => denied(error),
    }
}
/// Retain the accepted source on the same native creation transaction.
pub async fn retain(
    conn: &mut orch_db::Conn,
    row: &jobs::JobRow,
    origin: &Origin,
    accepted: &Caller,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    let recipe: Recipe = serde_json::from_value(row.plan["cascade"].clone())?;
    let made_here: bool =
        sqlx::query_scalar("SELECT xmin=pg_current_xact_id()::xid FROM jobs WHERE id=$1")
            .bind(row.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
    if !made_here
        || row.kind != KIND
        || row.state != "queued"
        || row.created_by != Some(origin.origin.user())
        || recipe.by != origin.origin.user()
        || row.plan.get(SOURCE).is_some()
    {
        return Err(denied(
            "the confirmation source does not own this new cascade",
        ));
    }
    let (caller, until) = origin.current(conn, clock).await?;
    let caller = caller.intersect_restrictions(accepted).map_err(denied)?;
    let caller = plan(conn, recipe.host)
        .await?
        .admit(&caller, false)
        .map_err(denied)?;
    let mut origin = origin.clone();
    origin.origin = CapturedOperation::capture(&caller).map_err(denied)?;
    origin.until = until;
    if let Some(http) = &origin.http {
        origin.http = Some(http.accepted(&caller, until).map_err(source_error)?);
    }
    let binding = Binding {
        origin,
        job: row.id,
        host: recipe.host,
        machine: recipe.machine,
        recipe: crate::jobs::command_identity::hash(&serde_json::to_value(recipe)?)?,
    };
    jobs::merge_plan(
        conn,
        row.id,
        &json!({SOURCE:binding,"lifecycle_owner":"host_delete","lifecycle_http_required":binding.origin.http.is_some()}),
        clock.now(),
    )
    .await?;
    Ok(())
}
/// The children retain their own logical acts. A parent's subsequent work controls this Host;
/// its current native managed/use collections are genuinely empty at that stage.
pub async fn principal(
    conn: &mut orch_db::Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Principal, EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        use sqlx::Connection as _;
        let mut tx = conn.begin().await.map_err(orch_db::DbError::from)?;
        let principal = principal_in(&mut tx, row, clock).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(principal);
    }
    principal_in(conn, row, clock).await
}
async fn principal_in(
    conn: &mut orch_db::Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Principal, EngineError> {
    let mut binding: Binding = serde_json::from_value(
        row.plan
            .get(SOURCE)
            .cloned()
            .ok_or_else(|| denied("the cascade source disappeared"))?,
    )?;
    if row.plan.get("lifecycle_http_required") == Some(&json!(true))
        && binding.origin.http.is_none()
    {
        return Err(denied("The original HTTP source disappeared."));
    }
    let recipe: Recipe = serde_json::from_value(row.plan["cascade"].clone())?;
    if row.id != binding.job
        || row.kind != KIND
        || row.created_by != Some(binding.origin.origin.user())
        || recipe.by != binding.origin.origin.user()
        || recipe.host != binding.host
        || recipe.machine != binding.machine
        || crate::jobs::command_identity::hash(&serde_json::to_value(&recipe)?)? != binding.recipe
    {
        return Err(denied("the cascade source association changed"));
    }
    let (caller, until) = binding.origin.current(conn, clock).await?;
    let host = hosts::host(conn, recipe.host)
        .await?
        .filter(|host| host.machine_id == recipe.machine)
        .ok_or_else(|| denied("the cascade Host changed"))?;
    let caller = plan(conn, host.id)
        .await?
        .admit(&caller, false)
        .map_err(denied)?;
    binding.origin.origin = CapturedOperation::capture(&caller).map_err(denied)?;
    binding.origin.until = until;
    if let Some(http) = &binding.origin.http {
        binding.origin.http = Some(http.accepted(&caller, until).map_err(source_error)?);
    }
    let principal = Principal::load(conn, caller, clock.time()).await?;
    if !crate::operation_access::review(
        conn,
        &principal,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host.id,
        },
        Permission::HostManage,
    )
    .await
    .map_err(denied)?
    .is_none()
    {
        return Err(denied("the confirmation no longer manages this Host"));
    }
    let narrowed = serde_json::to_value(binding)?;
    if row.plan.get(SOURCE) != Some(&narrowed) {
        jobs::merge_plan(conn, row.id, &json!({SOURCE:narrowed}), clock.now()).await?;
    }
    Ok(principal)
}
async fn plan(
    conn: &mut orch_db::Conn,
    host: Id,
) -> Result<crate::operation_scope::Prepared, EngineError> {
    let facts = vec![
        Facts::one(
            Source::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host,
                },
            )
            .await
            .map_err(denied)?,
        ),
        Facts::collection(Source::Managed, Vec::new()),
        Facts::collection(Source::Used, Vec::new()),
    ];
    sources::bind_required(HostDeleteCreate::ROUTE, &CREATE, &facts).map_err(denied)
}

/// Earliest retained real credential expiry, independent of a native review's expiry.
pub(crate) fn deadline(row: &jobs::JobRow) -> Result<Option<orch_types::Time>, EngineError> {
    let binding: Binding = serde_json::from_value(
        row.plan
            .get(SOURCE)
            .cloned()
            .ok_or_else(|| EngineError::unreadable("the native source disappeared"))?,
    )?;
    Ok(binding.origin.until)
}

/// The actual retained HTTP source, read only after this parent's current native check.
pub(crate) fn retained_http(
    row: &jobs::JobRow,
) -> Result<crate::operation_scope::http_origin::HttpOrigin, EngineError> {
    let binding: Binding = serde_json::from_value(
        row.plan
            .get(SOURCE)
            .cloned()
            .ok_or_else(|| denied("the Host source disappeared"))?,
    )
    .map_err(|_| denied("the Host source is invalid"))?;
    binding
        .origin
        .http
        .ok_or_else(|| denied("the Host child needs its actual HTTP source"))
}
