//! The provider creator's closed duplicate case uses the common continuing frame and raw carrier.
use super::*;
use crate::operation_scope::{
    continuing::{self, Source as NativeSource},
    http_origin::HttpOrigin,
    sources::Facts,
};
use futures::future::BoxFuture;
use orch_api_contract::{
    duplicate_sources::{PROVIDER, PROVIDER_MOVE, Source},
    duplicates::{ProviderDuplicate, ProviderMoveOut},
    routes::{Endpoint, Route},
};
use orch_types::Clock;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Origin {
    pub source: Id,
    pub raw: HttpOrigin,
    pub carries_secrets: bool,
    pub copy_keeper: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moving: Option<Hash>,
}
struct Duplicate<const MOVING: bool>;
struct Input {
    job: Id,
    source: Id,
    provider: Id,
    home: Id,
    carries_secrets: bool,
    copy_keeper: Option<Id>,
    moving: Option<Hash>,
}
struct Value {
    source_home: Id,
    org: Id,
    agent: Option<Id>,
    machines: Vec<Id>,
    owned_homes: bool,
}
fn target_error(error: targets::TargetError) -> continuing::Error {
    match error {
        targets::TargetError::Missing => continuing::Error::Refused,
        targets::TargetError::Db(error) => error.into(),
        targets::TargetError::Authz(error) => error.into(),
    }
}
pub(super) fn error(error: continuing::Error) -> EngineError {
    let category = match &error {
        continuing::Error::Refused => "native_source",
        continuing::Error::Binding(crate::operation_scope::BindingError::Refused) => "scope",
        continuing::Error::Binding(crate::operation_scope::BindingError::Catalogue(_)) => {
            "declaration"
        }
        continuing::Error::Native(_) => "native_error",
    };
    tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.checkpoint", category,
        "native provider copy checkpoint refused");
    match error {
        continuing::Error::Native(error) => error,
        _ => EngineError::unreadable("The provider copy source is no longer authorized."),
    }
}
impl<const MOVING: bool> NativeSource for Duplicate<MOVING> {
    type Role = Source;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = if MOVING {
        ProviderMoveOut::ROUTE
    } else {
        ProviderDuplicate::ROUTE
    };
    const DECLARATION: &'static orch_api_contract::operation_sources::Declaration<Source> =
        if MOVING { &PROVIDER_MOVE } else { &PROVIDER };
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a orch_authz::Caller,
        input: &'a Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Source>>), continuing::Error>> {
        Box::pin(async move {
            let source = orch_db::queries::resources::routes::providers(conn, &[input.source])
                .await?
                .pop()
                .ok_or(continuing::Error::Refused)?;
            if MOVING != input.moving.is_some() {
                return Err(continuing::Error::Refused);
            }
            if let Some(fingerprint) = input.moving
                && crate::providers::deleting::fingerprint(conn, &source).await? != fingerprint
            {
                return Err(continuing::Error::Refused);
            }
            let destination =
                orch_db::queries::resources::routes::providers(conn, &[input.provider])
                    .await?
                    .pop()
                    .ok_or(continuing::Error::Refused)?;
            if input.copy_keeper.is_some_and(|keeper| {
                (!matches!(source.kind.as_str(), "api" | "subscription")
                    || source.kind == "subscription" && !MOVING)
                    || source.key_machine_id.or(source.machine_id) != Some(keeper)
            }) {
                return Err(continuing::Error::Refused);
            }
            if input.source == input.provider || destination.home_id != input.home {
                return Err(continuing::Error::Refused);
            }
            let subject = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: input.source,
                },
            )
            .await
            .map_err(target_error)?;
            let home = targets::node(conn, input.home)
                .await
                .map_err(target_error)?;
            if !matches!(
                home.object.kind,
                KeyTargetKind::Object(ObjectKind::Org | ObjectKind::Project)
            ) {
                return Err(continuing::Error::Refused);
            }
            let mut machines: Vec<Id> = destination
                .machine_id
                .into_iter()
                .chain(destination.key_machine_id)
                .collect();
            machines.extend(input.copy_keeper);
            machines.sort_unstable();
            machines.dedup();
            let owned_homes = destination.kind != "subscription"
                || (orch_settings::provider_home_choice(conn, input.provider)
                    .await
                    .map_err(EngineError::from)?
                    == Some(orch_driver_contract::runtime::EnvironmentDataSource::MadeByOrch)
                    && orch_settings::credential_home_choice(conn, input.provider)
                        .await
                        .map_err(EngineError::from)?
                        == Some(orch_driver_contract::runtime::EnvironmentDataSource::MadeByOrch));
            Ok((
                Value {
                    source_home: source.home_id,
                    org: source.org_id,
                    agent: destination.agent_id,
                    machines,
                    owned_homes,
                },
                vec![
                    Facts::one(
                        Source::Subject,
                        KeyTargetKind::Object(ObjectKind::Provider),
                        subject,
                    ),
                    Facts::one(
                        Source::Destination,
                        KeyTargetKind::Object(ObjectKind::Provider),
                        home,
                    ),
                ],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async move {
            crate::copy_out::require_continuing(
                conn,
                principal,
                crate::copy_out::Subject::Object(ObjectRef {
                    kind: ObjectKind::Provider,
                    id: input.source,
                }),
                Some(input.home),
            )
            .await?;
            if !value.owned_homes {
                tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.require",
                    reason = "owned_homes", "native provider copy source closed");
                return Err(continuing::Error::Refused);
            }
            for machine in &value.machines {
                let current = orch_db::queries::resources::machine(conn, *machine).await?;
                if !current
                    .is_some_and(|machine| machine.deleted_at.is_none() && machine.is_active())
                    || !resources::check_machine(
                        conn,
                        principal,
                        *machine,
                        Permission::MachineUse,
                        Policy::default(),
                    )
                    .await?
                    .is_allowed()
                {
                    tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.require",
                        reason = "physical_use", "native provider copy source closed");
                    return Err(continuing::Error::Refused);
                }
            }
            if (input.carries_secrets || MOVING)
                && !resources::effective(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id: input.source,
                    },
                    Policy::default(),
                )
                .await?
                .owner
            {
                tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.require",
                    reason = "source_owner", "native provider copy source closed");
                return Err(continuing::Error::Refused);
            }
            if MOVING {
                if crate::providers::deleting::pending_move(conn, input.source, Some(input.job))
                    .await?
                    .is_some()
                {
                    tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.require",
                        reason = "source_move_pending",
                        "native provider copy source closed");
                    return Err(continuing::Error::Refused);
                }
                let destination = targets::node(conn, input.home)
                    .await
                    .map_err(target_error)?;
                if destination.node_ancestors.iter().any(|node| {
                    node.kind == KeyTargetKind::Object(ObjectKind::Org)
                        && node.id == Some(value.org)
                }) {
                    return Err(continuing::Error::Refused);
                }
            }
            let mut needed = vec![
                (ObjectKind::Provider, input.source, Permission::ProviderView),
                (
                    ObjectKind::Provider,
                    input.provider,
                    Permission::ProviderManage,
                ),
            ];
            if MOVING {
                needed.push((
                    ObjectKind::Provider,
                    input.source,
                    Permission::ProviderManage,
                ));
            }
            if let Some(agent) = value.agent {
                needed.push((ObjectKind::Agent, agent, Permission::AgentUse));
            }
            for (kind, id, permission) in needed {
                if !resources::check(
                    conn,
                    principal,
                    ObjectRef { kind, id },
                    permission,
                    Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.require",
                        reason = "account_permission", permission = permission.as_str(),
                        "native provider copy source closed");
                    return Err(continuing::Error::Refused);
                }
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async move {
            let ids: Vec<Id> = sqlx::query_scalar("SELECT id FROM providers WHERE id=ANY($1) AND deleted_at IS NULL ORDER BY id FOR SHARE")
                .bind(vec![input.source, input.provider]).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            if ids.len() != 2 {
                return Err(continuing::Error::Refused);
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, continuing::Error>> {
        Box::pin(async move {
            Ok(opened.source_home == fresh.source_home
                && opened.org == fresh.org
                && opened.agent == fresh.agent
                && opened.machines == fresh.machines
                && opened.owned_homes == fresh.owned_homes)
        })
    }
}
pub(super) async fn principal(
    conn: &mut Conn,
    authority: &Authority,
    clock: &dyn Clock,
) -> Result<Principal, EngineError> {
    let origin = authority
        .duplicate
        .as_ref()
        .ok_or_else(|| EngineError::unreadable("The provider copy source disappeared."))?;
    let operation = if origin.moving.is_some() {
        PROVIDER_MOVE.operation
    } else {
        PROVIDER.operation
    };
    if authority.origin.operation() != operation
        || origin.raw.origin().operation() != operation
        || origin.raw.origin().user() != authority.origin.user()
    {
        tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.origin",
            reason = "association", "native provider copy source closed");
        return Err(EngineError::unreadable("The provider copy origin changed."));
    }
    let original = authority
        .origin
        .restore_current(conn, clock)
        .await?
        .ok_or_else(|| {
            tracing::debug!(target: "orch_api::native_operations", phase = "provider.copy.origin",
                reason = "credential", "native provider copy source closed");
            EngineError::unreadable("The provider copy creator ended.")
        })?;
    let retained = origin
        .raw
        .with_ceiling(
            &original,
            original
                .grant()
                .and_then(orch_authz::TokenGrant::expires_at),
        )
        .map_err(error)?;
    let input = Input {
        job: authority.job,
        source: origin.source,
        provider: authority.provider,
        home: authority.home,
        carries_secrets: origin.carries_secrets,
        copy_keeper: origin.copy_keeper,
        moving: origin.moving,
    };
    let (principal, raw) = if origin.moving.is_some() {
        prepared::<true>(&retained, conn, clock, input).await?
    } else {
        prepared::<false>(&retained, conn, clock, input).await?
    };
    if raw != origin.raw {
        let mut next = authority.clone();
        next.duplicate
            .as_mut()
            .ok_or_else(|| EngineError::unreadable("The provider copy origin disappeared."))?
            .raw = raw.clone();
        jobs::merge_plan(conn, authority.job, &json!({FIELD:next}), clock.now()).await?;
    }
    if !principal.caller().key_deadline_allows(clock)
        || raw.until().is_some_and(|end| clock.time() >= end)
    {
        return Err(EngineError::unreadable("The provider copy lifetime ended."));
    }
    Ok(principal)
}
async fn prepared<const MOVING: bool>(
    raw: &HttpOrigin,
    conn: &mut Conn,
    clock: &dyn Clock,
    input: Input,
) -> Result<(Principal, HttpOrigin), EngineError> {
    let frame = raw
        .prepare::<Duplicate<MOVING>>(conn, clock, input)
        .await
        .map_err(error)?;
    let raw = frame
        .http_origin()
        .cloned()
        .ok_or_else(|| EngineError::unreadable("The provider copy raw origin disappeared."))?;
    let parts = frame.into_parts();
    parts.first_effect.check(clock).map_err(error)?;
    Ok((parts.principal, raw))
}
