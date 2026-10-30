//! Effective native catalogues are read from exact open sessions and never inferred from names or files.

use crate::commands::{Outcome, Outgoing};
use crate::unit::Unit;
use crate::{Engine, EngineError};
use orch_api_contract::input_schedules::NativeInvocation;
use orch_api_contract::native::{NativeAction, NativeDelivery, NativePermission};
use orch_api_contract::native::{
    NativeCatalog, NativeCatalogState, NativeOriginKind, PAGE_BYTES, SNAPSHOT_COMMANDS,
};
use orch_api_contract::threads::Delivery;
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{SerialKey, SessionNativeActions};
use orch_types::{Id, new_id};

/// A selected action is never silently replaced with an ordinary text input.
#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    /// The session no longer offers this exact selector or delivery path.
    #[error("the selected command is unavailable in this session")]
    Unavailable,
    /// The loaded mapping changed; the person's arguments remain theirs.
    #[error("the session's command list changed")]
    Stale {
        /// The revision which is current now.
        current_revision: String,
    },
    /// The supplied arguments do not meet the verified declaration.
    #[error("the arguments do not meet the command's declaration")]
    InvalidArguments,
    /// The author lacks the action's declared permission.
    #[error("the author lacks the command's required permission")]
    Denied(Box<orch_access_contract::decisions::Refusal>),
    /// Stored state or the database failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

struct LocalOnly;
impl jsonschema::Retrieve for LocalOnly {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<orch_types::Json, Box<dyn std::error::Error + Send + Sync>> {
        Err("a command's argument schema must resolve its references locally".into())
    }
}

/// Validates current binding, opaque id, declared authority/path and arguments in the input transaction.
pub async fn validate_selected(
    unit: &mut Unit<'_>,
    caller: &Caller,
    thread: Id,
    selection: &NativeInvocation,
    delivery: Delivery,
) -> Result<NativeAction, NativeError> {
    let stored: Option<(Option<Id>, Option<orch_types::Json>)> =
        sqlx::query_as("SELECT current_segment_id,native_catalog FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_optional(&mut *unit.conn)
            .await
            .map_err(DbError::from)
            .map_err(EngineError::from)?;
    let Some((Some(segment), Some(raw))) = stored else {
        return Err(NativeError::Unavailable);
    };
    let catalog: NativeCatalog =
        serde_json::from_value(raw).map_err(|_| NativeError::Unavailable)?;
    if catalog.state != NativeCatalogState::Ready {
        return Err(NativeError::Unavailable);
    }
    if catalog.revision != selection.revision
        || !catalog.revision.starts_with(&format!("{segment}:"))
    {
        return Err(NativeError::Stale {
            current_revision: catalog.revision,
        });
    }
    let action = catalog
        .actions
        .into_iter()
        .find(|action| action.id == selection.action_id && action.available)
        .ok_or(NativeError::Unavailable)?;
    let mode = match delivery {
        Delivery::Queue => NativeDelivery::Queue,
        Delivery::Steer => NativeDelivery::Steer,
        Delivery::Interrupt => NativeDelivery::Interrupt,
    };
    if !action.delivery_modes.contains(&mode) {
        return Err(NativeError::Unavailable);
    }
    let now = orch_types::Time::new(unit.now());
    let principal = Principal::load(&mut *unit.conn, caller.clone(), now)
        .await
        .map_err(EngineError::from)?;
    let permission = match action.permission {
        NativePermission::Interact => orch_access_contract::permissions::Permission::ThreadInteract,
        NativePermission::Manage => orch_access_contract::permissions::Permission::ThreadManage,
    };
    if let orch_access_contract::decisions::Decision::Refused(refusal) = orch_authz::nodes::check(
        &mut *unit.conn,
        &principal,
        thread,
        permission,
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?
    {
        return Err(NativeError::Denied(Box::new(refusal)));
    }
    let validator = jsonschema::options()
        .with_retriever(LocalOnly)
        .with_pattern_options(jsonschema::PatternOptions::regex())
        .should_validate_formats(true)
        .build(&action.arguments_schema)
        .map_err(|_| NativeError::Unavailable)?;
    if !validator.is_valid(&selection.arguments) {
        return Err(NativeError::InvalidArguments);
    }
    Ok(action)
}

/// A queued stale/unavailable selection keeps its prompt and arguments until its author changes it.
pub(crate) async fn hold_invalid(
    unit: &mut Unit<'_>,
    thread: Id,
    input: &orch_db::queries::threads::InputRow,
) -> Result<bool, EngineError> {
    let Some(raw) = &input.native_action else {
        return Ok(false);
    };
    let selection: NativeInvocation = serde_json::from_value(raw.clone())?;
    let Some(author) = input.author_id else {
        return Ok(true);
    };
    let delivery = match input.delivery.as_str() {
        "steer" => Delivery::Steer,
        "interrupt" => Delivery::Interrupt,
        _ => Delivery::Queue,
    };
    let caller = crate::inputs::input_author_caller(unit, thread, input)
        .await?
        .unwrap_or_else(|| {
            Caller::token(
                author,
                orch_authz::TokenGrant::new(
                    orch_access_contract::decisions::Scopes::Only(Default::default()),
                    None,
                ),
            )
        });
    match validate_selected(unit, &caller, thread, &selection, delivery).await {
        Ok(action) => {
            sqlx::query("UPDATE thread_inputs SET native_action_info=$2 WHERE id=$1")
                .bind(input.id).bind(serde_json::json!({"action_id":action.id,"kind":action.kind,"origin":action.origin}))
                .execute(&mut *unit.conn).await.map_err(DbError::from)?;
            Ok(false)
        }
        Err(NativeError::Internal(error)) => Err(error),
        Err(error) => {
            let reason = match error {
                NativeError::Stale { .. } => "native_action.stale",
                NativeError::Denied(_) => "native_action.permission",
                NativeError::InvalidArguments => "native_action.arguments",
                _ => "native_action.unavailable",
            };
            let seq = unit.stamp(thread).await?;
            orch_db::queries::threads::hold_input(&mut *unit.conn, input.id, Some(reason), seq)
                .await?;
            Ok(true)
        }
    }
}

/// A public revision is scoped to one segment; adapters still verify their own loaded revision.
#[must_use]
pub fn session_revision(segment: Id, revision: &str) -> String {
    format!("{segment}:{revision}")
}

/// Removes paths that cannot be safe workspace-relative provenance; metadata never holds skill contents.
fn safe(catalog: &mut NativeCatalog) {
    for action in &mut catalog.actions {
        if action.origin.path.as_ref().is_some_and(|path| {
            path.starts_with('/')
                || path.contains('\\')
                || path.split('/').any(|part| part == "..")
                || path.contains(':')
                || path.chars().any(char::is_control)
        }) {
            action.origin.path = None;
        }
        if action.origin.kind == NativeOriginKind::Project && !catalog.project_skills.available {
            action.available = false;
            action.reason = Some("Project skills are not loaded in this arrangement".to_owned());
        }
    }
}

#[derive(sqlx::FromRow)]
struct ActionSession {
    segment: Option<Id>,
    machine: Option<Id>,
    native: Option<String>,
    online: Option<bool>,
}

impl Engine {
    /// Reads an existing session only; no harness, lease, model request or turn is started.
    pub async fn native_actions(
        &self,
        thread: Id,
        refresh: bool,
    ) -> Result<NativeCatalog, EngineError> {
        let mut conn = self.db().acquire().await?;
        let source:Option<ActionSession>=sqlx::query_as(
            "SELECT t.current_segment_id AS segment,s.machine_id AS machine,s.native_session_ref AS native,m.online FROM threads t
             JOIN nodes n ON n.id=t.node_id LEFT JOIN segments s ON s.id=t.current_segment_id AND s.ended_at IS NULL
             LEFT JOIN machines m ON m.id=s.machine_id AND m.status='active' AND m.deleted_at IS NULL
             WHERE t.node_id=$1 AND n.deleted_at IS NULL")
            .bind(thread).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let Some(ActionSession {
            segment,
            machine,
            native,
            online,
        }) = source
        else {
            return Err(EngineError::unreadable("the thread is unavailable"));
        };
        let (Some(segment), Some(machine), Some(_), Some(true)) =
            (segment, machine, native, online)
        else {
            clear(&mut conn, thread).await?;
            return Ok(NativeCatalog::unavailable(
                NativeCatalogState::NotReady,
                "The session is not open on an online machine",
            ));
        };
        drop(conn);
        let command = Outgoing::new(
            new_id(),
            machine,
            SerialKey::Thread(thread),
            &SessionNativeActions {
                thread_id: thread,
                segment_id: segment,
                refresh,
            },
        )?;
        let mut catalog = match self.inner.commands.observe(command).await {
            Ok(Outcome::Succeeded(value)) => serde_json::from_value::<NativeCatalog>(value)
                .unwrap_or_else(|_| {
                    NativeCatalog::unavailable(
                        NativeCatalogState::Failed,
                        "The session's command list could not be read",
                    )
                }),
            _ => NativeCatalog::unavailable(
                NativeCatalogState::Failed,
                "The session's command list is unavailable",
            ),
        };
        safe(&mut catalog);
        let mut ids = std::collections::HashSet::new();
        if catalog.actions.len() > SNAPSHOT_COMMANDS
            || serde_json::to_vec(&catalog)?.len() > PAGE_BYTES
            || (catalog.state == NativeCatalogState::Ready
                && (catalog.revision.is_empty()
                    || catalog
                        .actions
                        .iter()
                        .any(|action| action.id.is_empty() || !ids.insert(action.id.clone()))))
        {
            catalog = NativeCatalog::unavailable(
                NativeCatalogState::Failed,
                "The session did not supply a complete selectable command list",
            );
        }
        let mut conn = self.db().acquire().await?;
        let unchanged:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads t JOIN machines m ON m.id=$3 WHERE t.node_id=$1 AND t.current_segment_id=$2 AND m.online AND m.status='active' AND m.deleted_at IS NULL)")
            .bind(thread).bind(segment).bind(machine).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if !unchanged {
            return Ok(NativeCatalog::unavailable(
                NativeCatalogState::NotReady,
                "The session changed while its commands were read",
            ));
        }
        let value = serde_json::to_value(&catalog)?;
        sqlx::query(
            "UPDATE threads SET native_catalog=$3 WHERE node_id=$1 AND current_segment_id=$2",
        )
        .bind(thread)
        .bind(segment)
        .bind(value)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
        Ok(catalog)
    }
}

async fn clear(conn: &mut Conn, thread: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET native_catalog=NULL WHERE node_id=$1")
        .bind(thread)
        .execute(conn)
        .await?;
    Ok(())
}

/// Writes one factual native dispatch for a started input, without copying its arguments.
pub(crate) async fn invoked(
    unit: &mut Unit<'_>,
    turn: &orch_db::queries::threads::TurnRow,
    input: Id,
) -> Result<(), EngineError> {
    let raw:Option<(Option<Id>,Option<orch_types::Json>)>=sqlx::query_as("SELECT author_id,native_action_info FROM thread_inputs WHERE id=$1 AND native_action IS NOT NULL")
        .bind(input).fetch_optional(&mut *unit.conn).await.map_err(DbError::from)?;
    let Some((author, Some(info))) = raw else {
        return Ok(());
    };
    let already:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE action='native_action.invoked' AND node_id=$1 AND details->>'input'=$2)")
        .bind(turn.thread_id).bind(input.to_string()).fetch_one(&mut *unit.conn).await.map_err(DbError::from)?;
    if already {
        return Ok(());
    }
    let mut entry = orch_db::audit::Entry::new(
        author.map_or(orch_db::audit::Actor::System, orch_db::audit::Actor::User),
        orch_db::audit::action!(orch_access_contract::audit::AuditAction::NativeActionInvoked)
            .as_str(),
        "thread",
        Some(turn.thread_id),
        "Invoked the selected native command",
    );
    entry.node_id = Some(turn.thread_id);
    entry.history = false;
    entry.details = serde_json::json!({"thread":turn.thread_id,"turn":turn.id,"input":input,"caller":author,
        "action_id":info.get("action_id"),"kind":info.get("kind"),"origin":info.get("origin")});
    let now = unit.now();
    orch_db::audit::append(
        &mut *unit.conn,
        &entry,
        &orch_db::audit::Origin::default(),
        now,
    )
    .await?;
    Ok(())
}
