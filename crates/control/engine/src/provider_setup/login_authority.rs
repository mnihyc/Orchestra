//! The same current Provider facts and ordinary management authority at sign-in continuations.

use super::Pending;
use crate::EngineError;
use crate::operation_scope::{BindingError, Prepared, targets};
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::resources::provider_runtime as rows;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

/// Private origin of the active attempt; no secrets, paths or reusable permission proof.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginAuthority {
    pub(super) attempt: Id,
    #[serde(default)]
    pub(super) origin: Option<CapturedOperation>,
}

impl std::fmt::Debug for LoginAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoginAuthority([private])")
    }
}

impl LoginAuthority {
    /// Captures the actual attempt before a request waits or sends another command.
    pub async fn load(conn: &mut Conn, provider: Id) -> Result<Option<Self>, EngineError> {
        if let Some(stored) = rows::login_authority(conn, provider).await? {
            return Ok(Some(serde_json::from_value(stored)?));
        }
        // Historical pending work still has a real attempt. Never fabricate one from state text.
        Ok(rows::state(conn, provider)
            .await?
            .and_then(|state| state.login_pending)
            .map(|bytes| serde_json::from_slice::<Pending>(&bytes))
            .transpose()?
            .map(|pending| Self {
                attempt: pending.attempt,
                origin: pending.authority,
            }))
    }

    pub(super) async fn current(&self, conn: &mut Conn, provider: Id) -> Result<bool, EngineError> {
        Ok(Self::load(conn, provider).await?.as_ref() == Some(self))
    }

    /// Recheck this exact attempt and its original credential before new continuation work.
    pub fn authorized<'a>(
        &'a self,
        conn: &'a mut Conn,
        provider: Id,
        now: Time,
    ) -> BoxFuture<'a, Result<bool, EngineError>> {
        Box::pin(async move {
            if !self.current(conn, provider).await? {
                return Ok(false);
            }
            match &self.origin {
                Some(origin) => provider_login_authorized(conn, provider, origin, now).await,
                None => Ok(true),
            }
        })
    }
}

/// Builds the complete single Provider effect using the same catalogue and factual reader as HTTP.
pub async fn login_operation(
    conn: &mut Conn,
    provider: Id,
    selection: &str,
) -> Result<Prepared, EngineError> {
    let mut plan = Prepared::for_selection(selection).map_err(EngineError::unreadable)?;
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    plan.bind(0, target.object.kind, &[target])
        .map_err(EngineError::unreadable)?;
    Ok(plan)
}

/// Restores accepted/current restrictions, then checks the account's existing ProviderManage.
/// This does not give a key browser inspection or As-admin authority.
pub fn provider_login_authorized<'a>(
    conn: &'a mut Conn,
    provider: Id,
    origin: &'a CapturedOperation,
    now: Time,
) -> BoxFuture<'a, Result<bool, EngineError>> {
    Box::pin(async move {
        if origin.operation() == "ProviderCreate" {
            // Inline sign-in is part of the actual creator. It keeps that selection,
            // and rechecks its current home plus ordinary ProviderManage before new work.
            return Ok(crate::jobs::provider_provision::creator_principal(
                conn, origin, provider, now,
            )
            .await?
            .is_some());
        }
        if !matches!(
            origin.operation(),
            "ProviderLoginStart.interactive"
                | "ProviderLoginStart.import"
                | "ProviderLoginInput"
                | "ProviderLoginCancel"
        ) {
            return Ok(false);
        }
        let Some(caller) = origin.restore(conn, now).await? else {
            return Ok(false);
        };
        let Some(provider_state) = rows::state(conn, provider)
            .await?
            .filter(|state| state.deleted_at.is_none())
        else {
            return Ok(false);
        };
        if crate::preflight::second_step_missing(conn, origin.user(), provider_state.home_id)
            .await?
        {
            return Ok(false);
        }
        let mut plan =
            Prepared::for_selection(origin.operation()).map_err(EngineError::unreadable)?;
        let object = ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        };
        let target = match targets::page_object(conn, object).await {
            Ok(target) => target,
            Err(targets::TargetError::Missing) => return Ok(false),
            Err(targets::TargetError::Authz(
                orch_authz::Error::NotFound(_)
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::Suspended { .. }
                | orch_authz::Error::Archived { .. },
            )) => return Ok(false),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        plan.bind(0, target.object.kind, &[target])
            .map_err(EngineError::unreadable)?;
        let caller = match plan.admit(&caller, false) {
            Ok(caller) => caller,
            Err(BindingError::Refused) => return Ok(false),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        let principal = match Principal::load(conn, caller, now).await {
            Ok(principal) => principal,
            Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        match orch_authz::resources::check(
            conn,
            &principal,
            object,
            Permission::ProviderManage,
            Policy::default(),
        )
        .await
        {
            Ok(decision) => Ok(decision.is_allowed()),
            Err(
                orch_authz::Error::NotFound(_)
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::Suspended { .. }
                | orch_authz::Error::Archived { .. },
            ) => Ok(false),
            Err(error) => Err(error.into()),
        }
    })
}

pub(super) async fn pending_current(
    conn: &mut Conn,
    provider: Id,
    pending: &Pending,
) -> Result<bool, EngineError> {
    let current = rows::state(conn, provider)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .and_then(|state| state.login_pending)
        .map(|bytes| serde_json::from_slice::<Pending>(&bytes))
        .transpose()?;
    Ok(current.as_ref().is_some_and(|current| {
        current.attempt == pending.attempt && current.authority == pending.authority
    }))
}

/// Immutable private evidence for one durable login command. The machine only receives its
/// ordinary command body; this carrier is checked at unacknowledged first-send/retry admission.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginCommandAuthority {
    provider: Id,
    source: LoginCommandSource,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum LoginCommandSource {
    // Reuse the existing private durable evidence carrier, without login-policy semantics.
    Handover {
        authority: Box<crate::credentials::handover_authority::HandoverCommandAuthority>,
    },
    Start {
        active: LoginAuthority,
        method: String,
    },
    Input {
        active: Option<LoginAuthority>,
        origin: Option<CapturedOperation>,
    },
    Cancel {
        origin: Option<CapturedOperation>,
    },
    Refresh {
        active: LoginAuthority,
    },
}

impl std::fmt::Debug for LoginCommandAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoginCommandAuthority([private])")
    }
}

impl LoginCommandAuthority {
    /// Credential return uses the same durable first-wire carrier and its own ordinary rules.
    #[must_use]
    pub fn handover(
        authority: crate::credentials::handover_authority::HandoverCommandAuthority,
    ) -> Self {
        Self {
            provider: authority.provider(),
            source: LoginCommandSource::Handover {
                authority: Box::new(authority),
            },
        }
    }

    /// Physical custody evidence survives the Engine process with its canonical command.
    #[must_use]
    pub fn handover_source(&self) -> Option<&crate::credentials::handover_authority::ReturnSource> {
        match &self.source {
            LoginCommandSource::Handover { authority } => Some(authority.source()),
            _ => None,
        }
    }

    /// Retain every REAL accepted login origin after the same ordinary canonical check.
    /// Historical/system None remains None; no operation or browser caller is invented.
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if let LoginCommandSource::Handover { authority } = &self.source {
            return authority.current_callers(conn, clock).await;
        }
        let mut origins = Vec::new();
        match &self.source {
            LoginCommandSource::Start { active, .. } | LoginCommandSource::Refresh { active } => {
                origins.extend(active.origin.iter());
            }
            LoginCommandSource::Input { active, origin } => {
                if let Some(active) = active {
                    origins.extend(active.origin.iter());
                }
                origins.extend(origin.iter());
            }
            LoginCommandSource::Cancel { origin } => origins.extend(origin.iter()),
            LoginCommandSource::Handover { authority } => {
                return authority.current_callers(conn, clock).await;
            }
        }
        let mut callers = Vec::with_capacity(origins.len());
        for origin in origins {
            let Some(caller) = origin.restore_current(conn, clock).await? else {
                return Ok(None);
            };
            callers.push((Some(caller), None));
        }
        Ok(Some(callers))
    }

    /// Canonical command identity is required for custody transfer evidence.
    #[expect(
        clippy::too_many_arguments,
        reason = "forward the canonical durable identity and actual wire body to the exact source checker"
    )]
    pub async fn permits_command(
        &self,
        conn: &mut Conn,
        command: Id,
        machine: Id,
        kind: &str,
        key: Option<&str>,
        body: &orch_types::Json,
        now: Time,
    ) -> Result<bool, EngineError> {
        if let LoginCommandSource::Handover { authority } = &self.source {
            if self.provider != authority.provider() {
                return Ok(false);
            }
            return authority
                .permits(conn, command, machine, kind, key, body, now)
                .await;
        }
        self.permits(conn, machine, kind, body, now).await
    }
    /// Retains the actual pending attempt, including an unclassified ordinary attempt.
    #[must_use]
    pub fn start(provider: Id, pending: &Pending) -> Self {
        Self {
            provider,
            source: LoginCommandSource::Start {
                active: LoginAuthority {
                    attempt: pending.attempt,
                    origin: pending.authority.clone(),
                },
                method: pending.method.clone(),
            },
        }
    }

    /// The answer's own accepted operation is independent of the active Start origin.
    #[must_use]
    pub fn input(
        provider: Id,
        active: Option<LoginAuthority>,
        origin: Option<CapturedOperation>,
    ) -> Self {
        Self {
            provider,
            source: LoginCommandSource::Input { active, origin },
        }
    }

    /// Cancellation authorizes cleanup independently; a later replacement must remain untouched.
    #[must_use]
    pub fn cancel(provider: Id, origin: Option<CapturedOperation>) -> Self {
        Self {
            provider,
            source: LoginCommandSource::Cancel { origin },
        }
    }

    /// A new observation caused by completion retains the same original attempt through waits.
    #[must_use]
    pub fn refresh(provider: Id, active: LoginAuthority) -> Self {
        Self {
            provider,
            source: LoginCommandSource::Refresh { active },
        }
    }

    /// Only these exact existing machine commands can carry this private evidence.
    #[must_use]
    pub fn command_kind(&self) -> &'static str {
        match &self.source {
            LoginCommandSource::Handover { authority } => authority.command_kind(),
            LoginCommandSource::Start { .. } => "provider.login.start",
            LoginCommandSource::Input { .. } => "provider.login.input",
            LoginCommandSource::Cancel { .. } => "provider.login.cancel",
            LoginCommandSource::Refresh { .. } => "provider.refresh",
        }
    }

    /// Reauthorize only the current canonical command, after reconnect/sealing and before wire
    /// publication. Already acknowledged commands keep the existing native-journal semantics.
    pub async fn permits(
        &self,
        conn: &mut Conn,
        machine: Id,
        kind: &str,
        body: &orch_types::Json,
        now: Time,
    ) -> Result<bool, EngineError> {
        if body.get("provider_id") != Some(&serde_json::json!(self.provider)) {
            return Ok(false);
        }
        let Some(row) = orch_db::queries::resources::thread_engine::provider(conn, self.provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(false);
        };
        if crate::leases::machine_of(&row) != Some(machine) {
            return Ok(false);
        }
        if kind != "provider.refresh"
            && (row.kind != "subscription"
                || body.get("agent_id") != row.agent_id.map(|id| serde_json::json!(id)).as_ref())
        {
            return Ok(false);
        }
        let current = LoginAuthority::load(conn, self.provider).await?;
        match &self.source {
            // A caller without the canonical ID/key cannot admit a return command.
            LoginCommandSource::Handover { .. } => return Ok(false),
            LoginCommandSource::Start { active, method } => {
                if kind != "provider.login.start"
                    || current.as_ref() != Some(active)
                    || body.get("method").and_then(orch_types::Json::as_str)
                        != Some(method.as_str())
                {
                    return Ok(false);
                }
                let pending = rows::state(conn, self.provider)
                    .await?
                    .and_then(|state| state.login_pending)
                    .map(|bytes| serde_json::from_slice::<Pending>(&bytes))
                    .transpose()?;
                let Some(pending) = pending.filter(|pending| {
                    pending.attempt == active.attempt
                        && pending.method == *method
                        && pending.authority == active.origin
                }) else {
                    return Ok(false);
                };
                let Some(agent) = row.agent_id else {
                    return Ok(false);
                };
                let Some(profile) =
                    orch_db::queries::resources::thread_engine::agent(conn, agent).await?
                else {
                    return Ok(false);
                };
                if super::check_pending(&profile, &pending).is_err() {
                    return Ok(false);
                }
                if !active.authorized(conn, self.provider, now).await? {
                    return Ok(false);
                }
            }
            LoginCommandSource::Input { active, origin } => {
                if kind != "provider.login.input" || &current != active {
                    return Ok(false);
                }
                if let Some(active) = active
                    && !active.authorized(conn, self.provider, now).await?
                {
                    return Ok(false);
                }
                if let Some(origin) = origin
                    && (origin.operation() != "ProviderLoginInput"
                        || !provider_login_authorized(conn, self.provider, origin, now).await?)
                {
                    return Ok(false);
                }
            }
            LoginCommandSource::Cancel { origin } => {
                if kind != "provider.login.cancel"
                    || current.is_some()
                    || rows::state(conn, self.provider)
                        .await?
                        .is_some_and(|state| state.login_pending.is_some())
                {
                    return Ok(false);
                }
                if let Some(origin) = origin
                    && (origin.operation() != "ProviderLoginCancel"
                        || !provider_login_authorized(conn, self.provider, origin, now).await?)
                {
                    return Ok(false);
                }
                return Ok(true);
            }
            LoginCommandSource::Refresh { active } => {
                if kind != "provider.refresh"
                    || current.as_ref() != Some(active)
                    || !active.authorized(conn, self.provider, now).await?
                {
                    return Ok(false);
                }
            }
        }
        Ok(orch_settings::provider_sign_in(conn, self.provider)
            .await?
            .allowed)
    }
}
