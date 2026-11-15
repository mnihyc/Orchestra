//! Public HTTP model calls keep their exact original provider, request and physical native association.

use super::{
    continuing::{self, Error, Source as NativeSource},
    sources::Facts,
    targets,
};
use crate::{EngineError, commands::Outgoing};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    operation_sources::Declaration,
    provider_relay_sources::{self, Source as Role},
    resources::*,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{
    Conn, DbError,
    queries::{provider_relay as rows, resources::thread_engine as resources},
};
use orch_link_contract::commands::{CommandKind, SerialKey};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Operation {
    Responses,
    Compact,
    InputTokens,
    Messages,
    MessageCount,
    Chat,
    Embeddings,
}

/// Only the native factory can capture this value; JSON arguments supply no authority.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallSource {
    origin: CapturedOperation,
    operation: Operation,
    call: Id,
    provider: Id,
    request_hash: String,
}
impl std::fmt::Debug for CallSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CallSource([private])")
    }
}
pub struct Responses;
pub struct Compact;
pub struct InputTokens;
pub struct Messages;
pub struct MessageCount;
pub struct Chat;
pub struct Embeddings;

async fn prepare(
    conn: &mut Conn,
    input: &CallSource,
    now: Time,
) -> Result<(rows::Call, Vec<Facts<Role>>), Error> {
    let call = rows::call(conn, input.call).await?.ok_or(Error::Refused)?;
    if call.provider_id != input.provider
        || call.request_hash != input.request_hash
        || call.finished_at.is_some()
        || call.deadline <= now.get()
        || serde_json::to_value(&input.origin).map_err(EngineError::unreadable)? != call.source
    {
        return Err(Error::Refused);
    }
    let provider = resources::provider(conn, call.provider_id)
        .await?
        .ok_or(Error::Refused)?;
    if call.machine_id.is_some() && crate::leases::machine_of(&provider) != call.machine_id {
        return Err(Error::Refused);
    }
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: call.provider_id,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    Ok((
        call,
        vec![Facts::one(
            Role::Provider,
            KeyTargetKind::Object(ObjectKind::Provider),
            target,
        )],
    ))
}
async fn require(conn: &mut Conn, principal: &Principal, call: &rows::Call) -> Result<(), Error> {
    if principal.user() != call.user_id {
        return Err(Error::Refused);
    }
    let selected = crate::provider_relay::select_frozen(
        conn,
        principal,
        call.provider_id,
        &call.model_name,
        &call.protocol,
        call.effort.as_deref(),
    )
    .await
    .map_err(|error| match error {
        crate::provider_relay::Error::Db(error) => Error::Native(error.into()),
        crate::provider_relay::Error::Authz(error) => Error::Native(error.into()),
        crate::provider_relay::Error::Internal(error) => Error::Native(error),
        _ => Error::Refused,
    })?;
    let frozen = call
        .model_parameters
        .get(&call.model_name)
        .ok_or(Error::Refused)?;
    if !crate::provider_relay::same_request_policy(frozen, &selected.parameters)? {
        return Err(Error::Refused);
    }
    Ok(())
}
macro_rules! reader {
    ($name:ident,$endpoint:ty,$declaration:ident) => {
        impl NativeSource for $name {
            type Role = Role;
            type Input = CallSource;
            type Value = rows::Call;
            const ROUTE: Route = <$endpoint>::ROUTE;
            const DECLARATION: &'static Declaration<Role> = &provider_relay_sources::$declaration;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                input: &'a CallSource,
                now: Time,
            ) -> BoxFuture<'a, Result<(rows::Call, Vec<Facts<Role>>), Error>> {
                Box::pin(prepare(conn, input, now))
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                _: &'a CallSource,
                value: &'a rows::Call,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(require(conn, principal, value))
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _: &'a Principal,
                input: &'a CallSource,
                _: &'a rows::Call,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
            }
            fn check_opened<'a>(
                _: &'a mut Conn,
                _: &'a Principal,
                _: &'a CallSource,
                old: &'a rows::Call,
                fresh: &'a rows::Call,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move {
                    Ok(old.id == fresh.id
                        && old.provider_id == fresh.provider_id
                        && old.machine_id == fresh.machine_id
                        && old.user_id == fresh.user_id
                        && old.request_hash == fresh.request_hash
                        && old.command_body == fresh.command_body)
                })
            }
        }
    };
}
reader!(Responses, ProviderRelayResponses, RESPONSES);
reader!(Compact, ProviderRelayCompact, COMPACT);
reader!(InputTokens, ProviderRelayInputTokens, INPUT_TOKENS);
reader!(Messages, ProviderRelayMessages, MESSAGES);
reader!(MessageCount, ProviderRelayMessageCount, MESSAGE_COUNT);
reader!(Chat, ProviderRelayChat, CHAT);
reader!(Embeddings, ProviderRelayEmbeddings, EMBEDDINGS);

impl CallSource {
    /// Before runtime preparation, the accepted call has no command or physical machine yet.
    pub fn initial(caller: &Caller, call: &rows::Call) -> Result<Self, EngineError> {
        if call.machine_id.is_some() || call.command_body != serde_json::json!({}) {
            return Err(EngineError::unreadable(
                "the model call already has a native command",
            ));
        }
        Self::from_caller(caller, call)
    }
    fn from_caller(caller: &Caller, call: &rows::Call) -> Result<Self, EngineError> {
        let operation = match caller.admitted_operation() {
            Some("ProviderRelayResponses") => Operation::Responses,
            Some("ProviderRelayCompact") => Operation::Compact,
            Some("ProviderRelayInputTokens") => Operation::InputTokens,
            Some("ProviderRelayMessages") => Operation::Messages,
            Some("ProviderRelayMessageCount") => Operation::MessageCount,
            Some("ProviderRelayChat") => Operation::Chat,
            Some("ProviderRelayEmbeddings") => Operation::Embeddings,
            _ => {
                return Err(EngineError::unreadable(
                    "the model call changed its original operation",
                ));
            }
        };
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        if caller.user() != call.user_id
            || serde_json::to_value(&origin).map_err(EngineError::unreadable)? != call.source
        {
            return Err(EngineError::unreadable(
                "the model call changed its original actor or source",
            ));
        }
        Ok(Self {
            origin,
            operation,
            call: call.id,
            provider: call.provider_id,
            request_hash: call.request_hash.clone(),
        })
    }
    /// Check both the same HTTP credential and the native provider before any new boundary.
    pub async fn check(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        raw: &super::http_origin::HttpOrigin,
    ) -> Result<(), Error> {
        macro_rules! check {
            ($maker:ty) => {{
                let frame = raw.prepare::<$maker>(conn, clock, self.clone()).await?;
                frame.check_deadline(clock)?;
            }};
        }
        match self.operation {
            Operation::Responses => check!(Responses),
            Operation::Compact => check!(Compact),
            Operation::InputTokens => check!(InputTokens),
            Operation::Messages => check!(Messages),
            Operation::MessageCount => check!(MessageCount),
            Operation::Chat => check!(Chat),
            Operation::Embeddings => check!(Embeddings),
        }
        Ok(())
    }
    pub fn capture(
        caller: &Caller,
        call: &rows::Call,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let operation = match caller.admitted_operation() {
            Some("ProviderRelayResponses") => Operation::Responses,
            Some("ProviderRelayCompact") => Operation::Compact,
            Some("ProviderRelayInputTokens") => Operation::InputTokens,
            Some("ProviderRelayMessages") => Operation::Messages,
            Some("ProviderRelayMessageCount") => Operation::MessageCount,
            Some("ProviderRelayChat") => Operation::Chat,
            Some("ProviderRelayEmbeddings") => Operation::Embeddings,
            _ => {
                return Err(EngineError::unreadable(
                    "the public model call changed its original operation",
                ));
            }
        };
        if command.id != call.id
            || Some(command.machine) != call.machine_id
            || command.key != SerialKey::None
            || command.kind != CommandKind::ProviderRelay
            || command.body != call.command_body
            || caller.user() != call.user_id
        {
            return Err(EngineError::unreadable(
                "the public model call changed its admitted native operands",
            ));
        }
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        if serde_json::to_value(&origin).map_err(EngineError::unreadable)? != call.source {
            return Err(EngineError::unreadable(
                "the public model call changed its original source",
            ));
        }
        Ok(Self {
            origin,
            operation,
            call: call.id,
            provider: call.provider_id,
            request_hash: call.request_hash.clone(),
        })
    }
    pub fn provider(&self) -> Id {
        self.provider
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn command_kind(&self) -> &'static str {
        "provider.relay"
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let _ = rows::lock_call(conn, self.call).await?;
        sqlx::query("SELECT id FROM providers WHERE id=$1 FOR SHARE")
            .bind(self.provider)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        Ok(())
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        macro_rules! frame {
            ($maker:ty) => {
                continuing::prepare::<$maker>(conn, clock, &self.origin, self.clone())
                    .await
                    .map(|frame| {
                        (
                            frame.principal().caller().clone(),
                            frame.check_deadline(clock),
                        )
                    })
            };
        }
        let current = match self.operation {
            Operation::Responses => frame!(Responses),
            Operation::Compact => frame!(Compact),
            Operation::InputTokens => frame!(InputTokens),
            Operation::Messages => frame!(Messages),
            Operation::MessageCount => frame!(MessageCount),
            Operation::Chat => frame!(Chat),
            Operation::Embeddings => frame!(Embeddings),
        };
        match current {
            Ok((caller, deadline)) => {
                deadline.map_err(|error| EngineError::BeforeEffect(Box::new(error)))?;
                Ok(Some(vec![(Some(caller), None)]))
            }
            Err(Error::Refused | Error::Binding(_)) => Ok(None),
            Err(Error::Native(error)) => Err(error),
        }
    }
    pub async fn current_issue(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_command(conn, command, clock, true).await
    }
    async fn current_command(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
        check_cap: bool,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let Some(call) = rows::call(conn, self.call).await? else {
            return Ok(None);
        };
        if command.id != call.id
            || Some(command.machine_id) != call.machine_id
            || command.kind != self.command_kind()
            || command.body != &call.command_body
            || command.serial_key.is_some()
            || command.content_transit
            || command.deadline_at != call.deadline
        {
            return Ok(None);
        }
        if check_cap
            && !crate::caps::own_room(
                conn,
                &crate::caps::OwnCapAsk {
                    place: call.org_id,
                    provider: call.provider_id,
                    model: &call.model_name,
                    tier: call
                        .headers
                        .get("cap_tier")
                        .and_then(serde_json::Value::as_str),
                    purpose: orch_db::usage::ledger::purpose::RELAY,
                    person: Some(call.user_id),
                },
                clock.now(),
            )
            .await?
        {
            return Ok(None);
        }
        self.current(conn, clock).await
    }
    pub async fn current_stored(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_command(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                content_transit: command.content_transit,
                deadline_at: command.deadline_at,
            },
            clock,
            false,
        )
        .await
    }
}
