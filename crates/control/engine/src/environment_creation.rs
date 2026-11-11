//! Native standalone creation bounds over actual template, mount and placement sources.

use crate::{
    EngineError,
    environments::CreateError,
    operation_scope::{BindingError, Prepared, sources, targets},
    places::HostPlace,
};
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    environment_sources::{
        CREATE, CreateSource as Source,
        duplicating::{self, CONNECTED as CONNECTED_COPY, MANAGED as DUPLICATE, MOVE_MANAGED},
    },
    operations::FacetUsage,
};
use orch_authz::Caller;
use orch_db::Conn;
use orch_types::Id;

use crate::operation_scope::continuing::{self, Error as NativeError, Source as NativeSource};
use futures::future::BoxFuture;
use orch_api_contract::{
    duplicates::{EnvironmentDuplicate, EnvironmentMoveOut},
    environment::EnvironmentCreate,
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Act, CapturedOperation, Policy, Principal};
use orch_db::queries::{environments::hosting as envs, jobs, resources::hosting as hosts};
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};
mod connected;

/// The standalone case uses the existing private creator-provision channel without a Thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Standalone {
    standalone: bool,
    origin: CapturedOperation,
    environment: Id,
    home: Id,
    job: Id,
    template: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    machine: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    configuration: Option<orch_types::Hash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    copy: Option<orch_types::Hash>,
}

/// Explicit standalone evidence never falls back to the historical Thread creator.
pub(crate) fn standalone(row: &jobs::JobRow) -> bool {
    row.plan.pointer("/creator_provision/standalone").is_some()
}

/// Factual association of this pending runtime-free child, without restoring its actor.
pub(crate) async fn connected_target(
    conn: &mut Conn,
    row: &jobs::JobRow,
) -> Result<
    Option<(
        orch_db::queries::environments::thread_engine::EnvironmentRow,
        Id,
    )>,
    EngineError,
> {
    if row
        .plan
        .pointer("/creator_provision/origin/operation")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|operation| !duplicating::is_connected(operation))
    {
        return Ok(None);
    }
    let proof = proof(row)?;
    let machine = proof
        .machine
        .ok_or_else(|| EngineError::unreadable("the connected copy has no actual machine"))?;
    let env = orch_db::queries::environments::thread_engine::environment(conn, proof.environment)
        .await?
        .filter(|env| {
            env.placement == "connected"
                && env.deleted_at.is_none()
                && env.remove_at.is_none()
                && env.template_id.is_none()
                && env.machine_id == Some(machine)
                && env.generation == 0
                && env.home_node_id == proof.home
                && env.owner_id == proof.origin.user()
                && env.status == "provisioning"
        })
        .ok_or_else(|| EngineError::unreadable("the connected copy target changed"))?;
    if Some(crate::jobs::command_identity::hash(&env.config)?) != proof.configuration {
        return Err(EngineError::unreadable(
            "the connected copy changed its owned-data recipe",
        ));
    }
    Ok(Some((env, machine)))
}

/// Bind only a genuinely new standalone native provision to its fully accepted operation.
pub async fn retain(
    conn: &mut Conn,
    origin: &CapturedOperation,
    made: &crate::environments::Made,
    home: Id,
    at: Time,
) -> Result<(), EngineError> {
    if origin.operation() != CREATE.operation {
        return Err(EngineError::unreadable(
            "this standalone source is not EnvironmentCreate",
        ));
    }
    retain_inner(conn, origin, made, home, at).await
}

/// The same native creator family retains a duplicate's actual raw identity and initial minima.
pub async fn retain_duplicate(
    conn: &mut Conn,
    origin: &CapturedOperation,
    raw: &crate::operation_scope::http_origin::HttpOrigin,
    made: &crate::environments::Made,
    home: Id,
    at: Time,
) -> Result<(), EngineError> {
    if !duplicating::is_copy(origin.operation())
        || raw.origin().operation() != origin.operation()
        || raw.origin().user() != origin.user()
    {
        return Err(EngineError::unreadable("the copy has another HTTP source"));
    }
    retain_inner(conn, origin, made, home, at).await?;
    jobs::merge_plan(
        conn,
        made.job.id,
        &serde_json::json!({"duplicate_http_origin":raw}),
        at.get(),
    )
    .await?;
    Ok(())
}

async fn retain_inner(
    conn: &mut Conn,
    origin: &CapturedOperation,
    made: &crate::environments::Made,
    home: Id,
    at: Time,
) -> Result<(), EngineError> {
    let row = jobs::lock(conn, made.job.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the standalone provision disappeared"))?;
    let env = envs::environment(conn, made.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the new Environment disappeared"))?;
    let connected = duplicating::is_connected(origin.operation());
    if (origin.operation() != CREATE.operation
        && !duplicating::is_managed(origin.operation())
        && !connected)
        || row.kind != jobs::kind::ENVIRONMENT_PROVISION
        || row.state != "queued"
        || row.environment_id != Some(env.id)
        || row.created_by != Some(origin.user())
        || env.owner_id != origin.user()
        || env.home_node_id != home
        || env.generation != 0
        || env.deleted_at.is_some()
        || row.plan["mode"] != if connected { "copy" } else { "make" }
        || (env.placement == "connected") != connected
        || row.plan["generation"] != 0
        || row
            .plan
            .get("runtime")
            .is_some_and(|value| !value.is_null())
        || row.plan.get("creator_provision").is_some()
    {
        return Err(EngineError::unreadable(
            "the standalone creator does not own this exact new provision",
        ));
    }
    let proof = Standalone {
        standalone: true,
        origin: origin.clone(),
        environment: env.id,
        home,
        job: row.id,
        template: if connected {
            None
        } else {
            Some(
                env.template_id
                    .ok_or_else(|| EngineError::unreadable("the new Template disappeared"))?,
            )
        },
        machine: if connected {
            Some(
                env.machine_id
                    .ok_or_else(|| EngineError::unreadable("the new Machine disappeared"))?,
            )
        } else {
            None
        },
        configuration: if connected {
            Some(crate::jobs::command_identity::hash(&env.config)?)
        } else {
            None
        },
        copy: if duplicating::is_copy(origin.operation()) {
            let initial: crate::initial_files::Plan =
                serde_json::from_value(row.plan["initial_files"].clone())?;
            if initial.source_thread().is_some() {
                return Err(EngineError::unreadable(
                    "the copy needs its actual Environment workspace",
                ));
            }
            Some(initial.source_stamp()?)
        } else {
            None
        },
    };
    jobs::merge_plan(
        conn,
        row.id,
        &serde_json::json!({"creator_provision":proof,"creator_provision_version":3}),
        at.get(),
    )
    .await?;
    Ok(())
}
fn proof(row: &jobs::JobRow) -> Result<Standalone, EngineError> {
    let proof: Standalone = serde_json::from_value(row.plan["creator_provision"].clone())?;
    let connected = duplicating::is_connected(proof.origin.operation());
    if row
        .plan
        .get("creator_provision_version")
        .is_some_and(|value| value != &serde_json::json!(3))
        || !proof.standalone
        || (proof.origin.operation() != CREATE.operation
            && !duplicating::is_managed(proof.origin.operation())
            && !connected)
        || ((duplicating::is_managed(proof.origin.operation()) || connected)
            != proof.copy.is_some())
        || connected != proof.machine.is_some()
        || connected != proof.configuration.is_some()
        || connected == proof.template.is_some()
        || proof.job != row.id
        || row.environment_id != Some(proof.environment)
        || row.created_by != Some(proof.origin.user())
        || row.kind != jobs::kind::ENVIRONMENT_PROVISION
        || !row.is_open()
        || row.cancel_requested()
        || row.plan["mode"] != if connected { "copy" } else { "make" }
        || row.plan["generation"] != 0
    {
        return Err(EngineError::unreadable(
            "the standalone creator binding changed",
        ));
    }
    Ok(proof)
}
struct Input {
    job: Id,
    origin: CapturedOperation,
    host: Option<Id>,
    copy: Option<orch_types::Hash>,
}
struct Value {
    environment: Id,
    home: Id,
    template: Id,
    host: Option<Id>,
    mounts: Vec<Id>,
    initial: Option<crate::initial_files::Plan>,
    duplicate: bool,
    copy: Option<orch_types::Hash>,
}
struct Choosing;
struct Placed;
struct DuplicateChoosing;
struct DuplicatePlaced;
struct MoveChoosing;
struct MovePlaced;
/// The same canonical source description before a native Host has been selected.
pub(crate) async fn known_plan(
    conn: &mut Conn,
    row: &jobs::JobRow,
) -> Result<Prepared, EngineError> {
    let proof = proof(row)?;
    let (_, mut facts) = native_prepare(
        conn,
        &Input {
            job: row.id,
            origin: proof.origin.clone(),
            host: None,
            copy: proof.copy,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    facts.retain(|fact| {
        matches!(
            fact.source,
            Source::Home
                | Source::InitialThreads
                | Source::InitialEnvironments
                | Source::MoveSubject
        )
    });
    if proof.origin.operation() == MOVE_MANAGED.operation {
        sources::bind_known(EnvironmentMoveOut::ROUTE, &MOVE_MANAGED, &facts)
            .map_err(EngineError::unreadable)
    } else if duplicating::is_managed(proof.origin.operation()) {
        sources::bind_known(EnvironmentDuplicate::ROUTE, &DUPLICATE, &facts)
            .map_err(EngineError::unreadable)
    } else {
        sources::bind_known(EnvironmentCreate::ROUTE, &CREATE, &facts)
            .map_err(EngineError::unreadable)
    }
}
// The native effect-free placement phase derives its known bindings from the one canonical row.
const CHOOSING: Declaration<Source> = Declaration {
    operation: CREATE.operation,
    bindings: &[
        CREATE.bindings[0],
        CREATE.bindings[1],
        CREATE.bindings[3],
        CREATE.bindings[4],
        CREATE.bindings[5],
    ],
};
const DUPLICATE_CHOOSING: Declaration<Source> = Declaration {
    operation: DUPLICATE.operation,
    bindings: &[
        DUPLICATE.bindings[0],
        DUPLICATE.bindings[1],
        DUPLICATE.bindings[3],
        DUPLICATE.bindings[4],
        DUPLICATE.bindings[5],
    ],
};
async fn native_prepare(
    conn: &mut Conn,
    input: &Input,
) -> Result<(Value, Vec<sources::Facts<Source>>), NativeError> {
    let row = jobs::job(conn, input.job)
        .await?
        .ok_or(NativeError::Refused)?;
    let proof = proof(&row)?;
    if proof.origin != input.origin || proof.copy != input.copy {
        return Err(NativeError::Refused);
    }
    let template_id = proof.template.ok_or(NativeError::Refused)?;
    let env = envs::environment(conn, proof.environment)
        .await?
        .ok_or(NativeError::Refused)?;
    if env.owner_id != proof.origin.user()
        || env.home_node_id != proof.home
        || env.template_id != proof.template
        || env.generation != 0
        || env.deleted_at.is_some()
        || env.status != "provisioning"
    {
        return Err(NativeError::Refused);
    }
    let template = hosts::template(conn, template_id)
        .await?
        .filter(|template| {
            template.enabled && template.deleted_at.is_none() && template.kind == "environment"
        })
        .ok_or(NativeError::Refused)?;
    let wanted = crate::places::wanted(conn, env.home_node_id, template, &env.config).await?;
    let host = input.host.or_else(|| {
        row.plan
            .get("host")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| id.parse().ok())
    });
    let mut facts = vec![
        sources::Facts::one(
            Source::Home,
            KeyTargetKind::Object(ObjectKind::Environment),
            targets::node(conn, env.home_node_id)
                .await
                .map_err(|error| NativeError::Native(EngineError::unreadable(error)))?,
        ),
        sources::Facts::one(
            Source::Template,
            KeyTargetKind::Object(ObjectKind::Template),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Template,
                    id: template_id,
                },
            )
            .await
            .map_err(|error| NativeError::Native(EngineError::unreadable(error)))?,
        ),
    ];
    let mut mounts = Vec::new();
    for id in &wanted.mounts {
        mounts.push(
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id: *id,
                },
            )
            .await
            .map_err(|error| NativeError::Native(EngineError::unreadable(error)))?,
        );
    }
    facts.push(sources::Facts::collection(
        Source::Mounts,
        if mounts.is_empty() {
            Vec::new()
        } else {
            vec![(KeyTargetKind::Object(ObjectKind::Mount), mounts)]
        },
    ));
    if let Some(id) = host {
        facts.push(sources::Facts::one(
            Source::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id,
                },
            )
            .await
            .map_err(|error| NativeError::Native(EngineError::unreadable(error)))?,
        ));
    }
    let initial = row
        .plan
        .get("initial_files")
        .map(|value| serde_json::from_value::<crate::initial_files::Plan>(value.clone()))
        .transpose()
        .map_err(EngineError::from)?;
    if let Some(expected) = proof.copy
        && initial
            .as_ref()
            .map(|plan| plan.source_stamp())
            .transpose()?
            != Some(expected)
    {
        return Err(NativeError::Refused);
    }
    let (threads, environments) = if let Some(initial) = initial.as_ref() {
        (
            match initial.source_thread() {
                Some(thread) => vec![(
                    KeyTargetKind::Object(ObjectKind::Thread),
                    vec![
                        targets::node_identity(conn, thread)
                            .await
                            .map_err(|error| NativeError::Native(EngineError::unreadable(error)))?,
                    ],
                )],
                None => Vec::new(),
            },
            vec![(
                KeyTargetKind::Object(ObjectKind::Environment),
                vec![
                    targets::resource_identity(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: initial.source_environment(),
                        },
                    )
                    .await
                    .map_err(|error| NativeError::Native(EngineError::unreadable(error)))?,
                ],
            )],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    facts.push(sources::Facts::collection(Source::InitialThreads, threads));
    facts.push(sources::Facts::collection(
        Source::InitialEnvironments,
        environments,
    ));
    if let Some(review) = crate::environments::ending::moving_review(&row)? {
        facts.push(sources::Facts::one(
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
    }
    Ok((
        Value {
            environment: env.id,
            home: env.home_node_id,
            template: template_id,
            host,
            mounts: wanted.mounts,
            initial,
            duplicate: duplicating::is_managed(proof.origin.operation()),
            copy: proof.copy,
        },
        facts,
    ))
}
macro_rules! stage {
    ($type:ty,$route:ty,$declaration:ident) => {
        impl NativeSource for $type {
            type Role = Source;
            type Input = Input;
            type Value = Value;
            const ROUTE: Route = <$route>::ROUTE;
            const DECLARATION: &'static Declaration<Source> = &$declaration;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                input: &'a Input,
                _: Time,
            ) -> BoxFuture<'a, Result<(Value, Vec<sources::Facts<Source>>), NativeError>> {
                Box::pin(native_prepare(conn, input))
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                input: &'a Input,
                value: &'a Value,
            ) -> BoxFuture<'a, Result<(), NativeError>> {
                Box::pin(async move {
                    use orch_access_contract::permissions::{
                        NodePermission, Permission, PermissionName,
                    };
                    if !orch_authz::nodes::check_named(
                        conn,
                        principal,
                        value.home,
                        PermissionName::OfNode(NodePermission::Interact),
                        Act::Use,
                        Policy::default(),
                    )
                    .await?
                    .is_allowed()
                    {
                        return Err(NativeError::Refused);
                    }
                    if value.duplicate {
                        let initial = value.initial.as_ref().ok_or(NativeError::Refused)?;
                        if initial.source_thread().is_some() {
                            return Err(NativeError::Refused);
                        }
                        crate::initial_files::require_source(conn, principal, initial).await?;
                        crate::copy_out::require_continuing(
                            conn,
                            principal,
                            crate::copy_out::Subject::Object(ObjectRef {
                                kind: ObjectKind::Environment,
                                id: initial.source_environment(),
                            }),
                            Some(value.home),
                        )
                        .await?;
                    }
                    if duplicating::is_move(input.origin.operation()) {
                        let owner = jobs::job(conn, input.job)
                            .await?
                            .ok_or(NativeError::Refused)?;
                        crate::environments::ending::require_move(conn, principal, &owner).await?;
                    }
                    let mut required = vec![(
                        ObjectKind::Template,
                        value.template,
                        Permission::TemplateUse,
                    )];
                    required.extend(
                        value
                            .mounts
                            .iter()
                            .map(|id| (ObjectKind::Mount, *id, Permission::MountUse)),
                    );
                    if let Some(id) = value.host {
                        required.push((ObjectKind::Host, id, Permission::HostUse));
                    }
                    for (kind, id, permission) in required {
                        if !orch_authz::resources::check(
                            conn,
                            principal,
                            ObjectRef { kind, id },
                            permission,
                            Policy::default(),
                        )
                        .await?
                        .is_allowed()
                        {
                            return Err(NativeError::Refused);
                        }
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
                    jobs::lock(conn, input.job)
                        .await?
                        .ok_or(NativeError::Refused)?;
                    envs::lock_environment(conn, value.environment)
                        .await?
                        .ok_or(NativeError::Refused)?;
                    if value.duplicate {
                        let source = value.initial.as_ref().ok_or(NativeError::Refused)?;
                        orch_db::queries::environments::hosting::lock_environment(
                            conn,
                            source.source_environment(),
                        )
                        .await?
                        .ok_or(NativeError::Refused)?;
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
            ) -> BoxFuture<'a, Result<bool, NativeError>> {
                Box::pin(async move {
                    Ok(opened.environment == fresh.environment
                        && opened.home == fresh.home
                        && opened.template == fresh.template
                        && opened.copy == fresh.copy
                        && opened.host == fresh.host)
                })
            }
        }
    };
}
stage!(Choosing, EnvironmentCreate, CHOOSING);
stage!(Placed, EnvironmentCreate, CREATE);
stage!(DuplicateChoosing, EnvironmentDuplicate, DUPLICATE_CHOOSING);
stage!(DuplicatePlaced, EnvironmentDuplicate, DUPLICATE);
const MOVE_CHOOSING: Declaration<Source> = Declaration {
    operation: MOVE_MANAGED.operation,
    bindings: &[
        MOVE_MANAGED.bindings[0],
        MOVE_MANAGED.bindings[1],
        MOVE_MANAGED.bindings[3],
        MOVE_MANAGED.bindings[4],
        MOVE_MANAGED.bindings[5],
        MOVE_MANAGED.bindings[6],
    ],
};
stage!(MoveChoosing, EnvironmentMoveOut, MOVE_CHOOSING);
stage!(MovePlaced, EnvironmentMoveOut, MOVE_MANAGED);

/// Refresh the actual original/current operation before native placement or first wire.
pub(crate) async fn principal_current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
    host: Option<Id>,
) -> Result<Principal, EngineError> {
    checkpoint_current(conn, row, clock, host)
        .await
        .map(|(principal, _)| principal)
}

/// The same native frame retains the original raw deadline for the caller's later waits.
pub(crate) async fn checkpoint_current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
    host: Option<Id>,
) -> Result<(Principal, Option<Time>), EngineError> {
    let proof = proof(row)?;
    if duplicating::is_connected(proof.origin.operation()) {
        if host.is_some() {
            return Err(EngineError::unreadable(
                "a connected copy has no selected Host",
            ));
        }
        return connected::checkpoint(conn, row, clock, proof).await;
    }
    let input = Input {
        job: row.id,
        origin: proof.origin.clone(),
        host,
        copy: proof.copy,
    };
    if duplicating::is_managed(proof.origin.operation()) {
        let raw: crate::operation_scope::http_origin::HttpOrigin =
            serde_json::from_value(row.plan["duplicate_http_origin"].clone())?;
        if raw.origin().user() != proof.origin.user()
            || raw.origin().operation() != proof.origin.operation()
        {
            return Err(EngineError::unreadable(
                "the copy lost its original HTTP source",
            ));
        }
        let placed = host.is_some()
            || row
                .plan
                .get("host")
                .is_some_and(|host| host.as_str().is_some());
        if placed {
            return if duplicating::is_move(proof.origin.operation()) {
                copy_placed::<MovePlaced>(conn, row, clock, &raw, input).await
            } else {
                copy_placed::<DuplicatePlaced>(conn, row, clock, &raw, input).await
            };
        }
        return if duplicating::is_move(proof.origin.operation()) {
            copy_choosing::<MoveChoosing>(conn, row, clock, &raw, input).await
        } else {
            copy_choosing::<DuplicateChoosing>(conn, row, clock, &raw, input).await
        };
    }
    if host.is_some()
        || row
            .plan
            .get("host")
            .is_some_and(|host| host.as_str().is_some())
    {
        continuing::prepare::<Placed>(conn, clock, &proof.origin, input)
            .await
            .map(|frame| (frame.into_parts().principal, None))
            .map_err(EngineError::unreadable)
    } else {
        continuing::preparation::prepare::<Choosing>(conn, clock, &proof.origin, input)
            .await
            .map(|frame| (frame.into_parts().principal, None))
            .map_err(EngineError::unreadable)
    }
}
/// Legacy sampled-time callers retain their interface; wire checks use their actual Clock.
pub(crate) async fn principal_at(
    conn: &mut Conn,
    row: &jobs::JobRow,
    now: Time,
) -> Result<Principal, EngineError> {
    #[derive(Debug)]
    struct At(Time);
    impl Clock for At {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            self.0.get()
        }
    }
    principal_current(conn, row, &At(now), None).await
}

fn failed(error: BindingError) -> CreateError {
    match error {
        BindingError::Refused => CreateError::OperationScope,
        error => EngineError::unreadable(error).into(),
    }
}
fn indices(plan: &Prepared, source: Source) -> Result<Vec<usize>, CreateError> {
    let declaration = if plan.variant().operation == MOVE_MANAGED.operation {
        &MOVE_MANAGED
    } else if plan.variant().operation
        == orch_api_contract::environment_sources::duplicating::MOVE_MANAGED_PREVIEW.operation
    {
        &orch_api_contract::environment_sources::duplicating::MOVE_MANAGED_PREVIEW
    } else if plan.variant().operation == DUPLICATE.operation {
        &DUPLICATE
    } else {
        &CREATE
    };
    let binding = declaration
        .bindings
        .iter()
        .find(|binding| binding.source == source && binding.usage == FacetUsage::Required)
        .ok_or_else(|| EngineError::unreadable("the native creation source is not declared"))?;
    sources::indices(plan, binding).map_err(failed)
}
/// Narrow the existing native candidate order using its declared actual sources.
/// This grants no ordinary Host/Template/Mount permission and changes no ranking policy.
pub async fn bound_candidates(
    conn: &mut Conn,
    caller: &Caller,
    known: &Prepared,
    template: Id,
    mounts: &[Id],
    places: &mut Vec<HostPlace>,
) -> Result<(), CreateError> {
    if known.variant().operation != CREATE.operation
        && known.variant().operation != DUPLICATE.operation
        && known.variant().operation != MOVE_MANAGED.operation
        && known.variant().operation
            != orch_api_contract::environment_sources::duplicating::MOVE_MANAGED_PREVIEW.operation
    {
        return Err(
            EngineError::unreadable("another operation cannot supply creation bounds").into(),
        );
    }
    let mut plan = known.clone();
    let template = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Template,
            id: template,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    for index in indices(&plan, Source::Template)? {
        plan.bind(
            index,
            KeyTargetKind::Object(ObjectKind::Template),
            std::slice::from_ref(&template),
        )
        .map_err(failed)?;
    }
    let mut actual_mounts = Vec::new();
    for id in mounts {
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
    for index in indices(&plan, Source::Mounts)? {
        if actual_mounts.is_empty() {
            plan.bind_empty(index)
        } else {
            plan.bind_collection(
                index,
                &[(
                    KeyTargetKind::Object(ObjectKind::Mount),
                    actual_mounts.clone(),
                )],
            )
        }
        .map_err(failed)?;
    }
    plan.preparation_caller(caller).map_err(failed)?;
    let host_indices = indices(&plan, Source::Host)?;
    let mut allowed = Vec::new();
    let mut excluded = false;
    for place in places.drain(..) {
        let host = targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Host,
                id: place.host.id,
            },
        )
        .await
        .map_err(EngineError::unreadable)?;
        let mut permits = true;
        for index in &host_indices {
            permits &= plan
                .permits_binding(
                    caller,
                    *index,
                    KeyTargetKind::Object(ObjectKind::Host),
                    std::slice::from_ref(&host),
                )
                .map_err(failed)?;
        }
        if permits {
            allowed.push(place);
        } else {
            excluded = true;
        }
    }
    *places = allowed;
    if places.is_empty() && excluded {
        return Err(CreateError::OperationScope);
    }
    Ok(())
}

async fn copy_placed<F: NativeSource<Role = Source, Input = Input, Value = Value>>(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
    raw: &crate::operation_scope::http_origin::HttpOrigin,
    input: Input,
) -> Result<(Principal, Option<Time>), EngineError> {
    let frame = raw
        .prepare::<F>(conn, clock, input)
        .await
        .map_err(EngineError::unreadable)?;
    let retained = frame
        .http_origin()
        .cloned()
        .ok_or_else(|| EngineError::unreadable("the copy lost its raw source"))?;
    let parts = frame.into_parts();
    let until = retained.until();
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
async fn copy_choosing<F: NativeSource<Role = Source, Input = Input, Value = Value>>(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
    raw: &crate::operation_scope::http_origin::HttpOrigin,
    input: Input,
) -> Result<(Principal, Option<Time>), EngineError> {
    let frame = raw
        .scope(continuing::preparation::prepare::<F>(
            conn,
            clock,
            raw.origin(),
            input,
        ))
        .await
        .map_err(EngineError::unreadable)?;
    let retained = frame
        .http_origin()
        .cloned()
        .ok_or_else(|| EngineError::unreadable("the copy lost its raw source"))?;
    let parts = frame.into_parts();
    let until = retained.until();
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
