//! The runtime-free leaf of the same native standalone creator family.
use super::*;
use crate::operation_scope::http_origin::HttpOrigin;
use orch_access_contract::permissions::Permission;
use orch_authz::environments::{Creating, NewEnvironment, Place};
use orch_db::queries::{resources, transaction_guard};

struct Connected<const MOVING: bool>;
struct Input {
    proof: Standalone,
}
struct Value {
    env: envs::EnvironmentState,
    machine: resources::MachineRow,
    files: crate::initial_files::Plan,
    mounts: Vec<Id>,
}

impl<const MOVING: bool> NativeSource for Connected<MOVING> {
    type Role = Source;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = if MOVING {
        EnvironmentMoveOut::ROUTE
    } else {
        EnvironmentDuplicate::ROUTE
    };
    const DECLARATION: &'static Declaration<Source> = if MOVING {
        &duplicating::MOVE_CONNECTED
    } else {
        &CONNECTED_COPY
    };
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<sources::Facts<Source>>), NativeError>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.proof.job)
                .await?
                .ok_or(NativeError::Refused)?;
            let actual = proof(&row)?;
            if actual != input.proof || actual.origin.operation() != Self::DECLARATION.operation {
                return Err(NativeError::Refused);
            }
            let env = envs::environment(conn, actual.environment)
                .await?
                .ok_or(NativeError::Refused)?;
            let machine = resources::machine(conn, actual.machine.ok_or(NativeError::Refused)?)
                .await?
                .ok_or(NativeError::Refused)?;
            if env.placement != "connected"
                || env.machine_id != Some(machine.id)
                || env.owner_id != actual.origin.user()
                || env.home_node_id != actual.home
                || env.template_id.is_some()
                || env.generation != 0
                || env.deleted_at.is_some()
                || env.remove_at.is_some()
                || env.status != "provisioning"
                || !machine.is_active()
                || Some(crate::jobs::command_identity::hash(&env.config)?) != actual.configuration
            {
                return Err(NativeError::Refused);
            }
            let data: orch_driver_contract::runtime::EnvironmentDataSources =
                serde_json::from_value(
                    env.config
                        .get(orch_settings_contract::registry::keys::ENV_DATA_SOURCES)
                        .cloned()
                        .ok_or(NativeError::Refused)?,
                )
                .map_err(EngineError::from)?;
            if data != Default::default() {
                return Err(NativeError::Refused);
            }
            let files: crate::initial_files::Plan =
                serde_json::from_value(row.plan["initial_files"].clone())
                    .map_err(EngineError::from)?;
            if files.source_thread().is_some() || Some(files.source_stamp()?) != actual.copy {
                return Err(NativeError::Refused);
            }
            let mounts = crate::mounts::resolve(conn, env.id, now.get())
                .await?
                .into_iter()
                .map(|mount| mount.source)
                .collect::<Vec<_>>();
            let mut bound = vec![
                sources::Facts::one(
                    Source::Home,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    targets::node(conn, env.home_node_id)
                        .await
                        .map_err(EngineError::unreadable)?,
                ),
                sources::Facts::one(
                    Source::Machine,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    targets::machine_subject(conn, machine.id)
                        .await
                        .map_err(EngineError::unreadable)?,
                ),
                sources::Facts::collection(Source::InitialThreads, Vec::new()),
                sources::Facts::collection(
                    Source::InitialEnvironments,
                    vec![(
                        KeyTargetKind::Object(ObjectKind::Environment),
                        vec![
                            targets::resource_identity(
                                conn,
                                ObjectRef {
                                    kind: ObjectKind::Environment,
                                    id: files.source_environment(),
                                },
                            )
                            .await
                            .map_err(EngineError::unreadable)?,
                        ],
                    )],
                ),
            ];
            let mut actual_mounts = Vec::new();
            for id in &mounts {
                actual_mounts.push(
                    targets::resource_identity(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Mount,
                            id: *id,
                        },
                    )
                    .await
                    .map_err(EngineError::unreadable)?,
                );
            }
            bound.push(sources::Facts::collection(
                Source::Mounts,
                if actual_mounts.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Mount), actual_mounts)]
                },
            ));
            if let Some(review) = crate::environments::ending::moving_review(&row)? {
                if !MOVING {
                    return Err(NativeError::Refused);
                }
                bound.push(sources::Facts::one(
                    Source::MoveSubject,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    targets::resource_identity(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: review.source(),
                        },
                    )
                    .await
                    .map_err(EngineError::unreadable)?,
                ));
            } else if MOVING {
                return Err(NativeError::Refused);
            }
            Ok((
                Value {
                    env,
                    machine,
                    files,
                    mounts,
                },
                bound,
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), NativeError>> {
        Box::pin(async move {
            if MOVING {
                let owner = jobs::job(conn, input.proof.job)
                    .await?
                    .ok_or(NativeError::Refused)?;
                crate::environments::ending::require_move(conn, principal, &owner).await?;
            }
            crate::initial_files::require_source(conn, principal, &value.files).await?;
            crate::copy_out::require_continuing(
                conn,
                principal,
                crate::copy_out::Subject::Object(ObjectRef {
                    kind: ObjectKind::Environment,
                    id: value.files.source_environment(),
                }),
                Some(value.env.home_node_id),
            )
            .await?;
            for permission in [Permission::MachineView, Permission::MachineUse] {
                if !orch_authz::resources::check_machine(
                    conn,
                    principal,
                    value.machine.id,
                    permission,
                    Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    return Err(NativeError::Refused);
                }
            }
            let policies = crate::access::resource_policies(
                conn,
                value.machine.host_id.unwrap_or(value.machine.id),
            )
            .await?;
            let ask = NewEnvironment {
                home: value.env.home_node_id,
                place: Place::Machine {
                    machine: value.machine.id,
                },
                mounts: value.mounts.clone(),
            };
            if !matches!(
                orch_authz::environments::check_existing_placement(
                    conn,
                    principal,
                    &ask,
                    &policies,
                    value.env.id
                )
                .await?,
                Creating::Allowed(_)
            ) {
                return Err(NativeError::Refused);
            }
            let home = orch_access_contract::permissions::PermissionName::OfNode(
                orch_access_contract::permissions::NodePermission::Interact,
            );
            if !orch_authz::nodes::check_named(
                conn,
                principal,
                value.env.home_node_id,
                home,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(NativeError::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), NativeError>> {
        Box::pin(async move {
            jobs::lock(conn, input.proof.job)
                .await?
                .ok_or(NativeError::Refused)?;
            envs::lock_environment(conn, value.env.id)
                .await?
                .ok_or(NativeError::Refused)?;
            envs::lock_environment(conn, value.files.source_environment())
                .await?
                .ok_or(NativeError::Refused)?;
            transaction_guard::share_machine(conn, value.machine.id).await?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, NativeError>> {
        Box::pin(async move {
            Ok(opened.env.id == fresh.env.id
                && opened.env.home_node_id == fresh.env.home_node_id
                && opened.machine.id == fresh.machine.id
                && opened.machine.host_id == fresh.machine.host_id
                && opened.files.source_stamp()? == fresh.files.source_stamp()?)
        })
    }
}

pub(super) async fn checkpoint(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
    proof: Standalone,
) -> Result<(Principal, Option<Time>), EngineError> {
    if duplicating::is_move(proof.origin.operation()) {
        checkpoint_inner::<true>(conn, row, clock, proof).await
    } else {
        checkpoint_inner::<false>(conn, row, clock, proof).await
    }
}
async fn checkpoint_inner<const MOVING: bool>(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
    proof: Standalone,
) -> Result<(Principal, Option<Time>), EngineError> {
    let raw: HttpOrigin = serde_json::from_value(row.plan["duplicate_http_origin"].clone())?;
    if raw.origin().user() != proof.origin.user()
        || raw.origin().operation()
            != if MOVING {
                duplicating::MOVE_CONNECTED.operation
            } else {
                CONNECTED_COPY.operation
            }
    {
        return Err(EngineError::unreadable(
            "the connected copy lost its original HTTP source",
        ));
    }
    let frame = raw
        .prepare::<Connected<MOVING>>(conn, clock, Input { proof })
        .await
        .map_err(EngineError::unreadable)?;
    let retained = frame
        .http_origin()
        .cloned()
        .ok_or_else(|| EngineError::unreadable("the connected copy lost its raw minima"))?;
    let until = retained.until();
    let parts = frame.into_parts();
    jobs::merge_plan(
        conn,
        row.id,
        &serde_json::json!({"duplicate_http_origin":retained}),
        clock.now(),
    )
    .await?;
    parts
        .first_effect
        .check(clock)
        .map_err(EngineError::unreadable)?;
    Ok((parts.principal, until))
}
