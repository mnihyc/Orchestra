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
    environment_sources::{CREATE, CreateSource as Source},
    operations::FacetUsage,
};
use orch_authz::Caller;
use orch_db::Conn;
use orch_types::Id;

use crate::operation_scope::continuing::{self, Error as NativeError, Source as NativeSource};
use futures::future::BoxFuture;
use orch_api_contract::{
    environment::EnvironmentCreate,
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Act, CapturedOperation, Policy, Principal};
use orch_db::queries::{environments::hosting as envs, jobs, resources::hosting as hosts};
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};

/// The standalone case uses the existing private creator-provision channel without a Thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Standalone {
    standalone: bool,
    origin: CapturedOperation,
    environment: Id,
    home: Id,
    job: Id,
    template: Id,
}

/// Explicit standalone evidence never falls back to the historical Thread creator.
pub(crate) fn standalone(row: &jobs::JobRow) -> bool {
    row.plan.pointer("/creator_provision/standalone").is_some()
}

/// Bind only a genuinely new standalone native provision to its fully accepted operation.
pub async fn retain(
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
    if origin.operation() != CREATE.operation
        || row.kind != jobs::kind::ENVIRONMENT_PROVISION
        || row.state != "queued"
        || row.environment_id != Some(env.id)
        || row.created_by != Some(origin.user())
        || env.owner_id != origin.user()
        || env.home_node_id != home
        || env.generation != 0
        || env.deleted_at.is_some()
        || row.plan["mode"] != "make"
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
        template: env
            .template_id
            .ok_or_else(|| EngineError::unreadable("the new Template disappeared"))?,
    };
    jobs::merge_plan(
        conn,
        row.id,
        &serde_json::json!({"creator_provision":proof}),
        at.get(),
    )
    .await?;
    Ok(())
}
fn proof(row: &jobs::JobRow) -> Result<Standalone, EngineError> {
    let proof: Standalone = serde_json::from_value(row.plan["creator_provision"].clone())?;
    if !proof.standalone
        || proof.origin.operation() != CREATE.operation
        || proof.job != row.id
        || row.environment_id != Some(proof.environment)
        || row.created_by != Some(proof.origin.user())
        || row.kind != jobs::kind::ENVIRONMENT_PROVISION
        || !row.is_open()
        || row.cancel_requested()
        || row.plan["mode"] != "make"
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
}
struct Value {
    environment: Id,
    home: Id,
    template: Id,
    host: Option<Id>,
    mounts: Vec<Id>,
}
struct Choosing;
struct Placed;
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
            origin: proof.origin,
            host: None,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    facts.retain(|fact| {
        matches!(
            fact.source,
            Source::Home | Source::InitialThreads | Source::InitialEnvironments
        )
    });
    sources::bind_known(EnvironmentCreate::ROUTE, &CREATE, &facts).map_err(EngineError::unreadable)
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
async fn native_prepare(
    conn: &mut Conn,
    input: &Input,
) -> Result<(Value, Vec<sources::Facts<Source>>), NativeError> {
    let row = jobs::job(conn, input.job)
        .await?
        .ok_or(NativeError::Refused)?;
    let proof = proof(&row)?;
    if proof.origin != input.origin {
        return Err(NativeError::Refused);
    }
    let env = envs::environment(conn, proof.environment)
        .await?
        .ok_or(NativeError::Refused)?;
    if env.owner_id != proof.origin.user()
        || env.home_node_id != proof.home
        || env.template_id != Some(proof.template)
        || env.generation != 0
        || env.deleted_at.is_some()
        || env.status != "provisioning"
    {
        return Err(NativeError::Refused);
    }
    let template = hosts::template(conn, proof.template)
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
                    id: proof.template,
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
    let (threads, environments) = if let Some(initial) = initial {
        (
            vec![(
                KeyTargetKind::Object(ObjectKind::Thread),
                vec![
                    targets::node_identity(conn, initial.source_thread())
                        .await
                        .map_err(|error| NativeError::Native(EngineError::unreadable(error)))?,
                ],
            )],
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
    Ok((
        Value {
            environment: env.id,
            home: env.home_node_id,
            template: proof.template,
            host,
            mounts: wanted.mounts,
        },
        facts,
    ))
}
macro_rules! stage {
    ($type:ty,$declaration:ident) => {
        impl NativeSource for $type {
            type Role = Source;
            type Input = Input;
            type Value = Value;
            const ROUTE: Route = EnvironmentCreate::ROUTE;
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
                _: &'a Input,
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
                        && opened.host == fresh.host)
                })
            }
        }
    };
}
stage!(Choosing, CHOOSING);
stage!(Placed, CREATE);

/// Refresh the actual original/current operation before native placement or first wire.
pub(crate) async fn principal_current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
    host: Option<Id>,
) -> Result<Principal, EngineError> {
    let proof = proof(row)?;
    let input = Input {
        job: row.id,
        origin: proof.origin.clone(),
        host,
    };
    if host.is_some()
        || row
            .plan
            .get("host")
            .is_some_and(|host| host.as_str().is_some())
    {
        continuing::prepare::<Placed>(conn, clock, &proof.origin, input)
            .await
            .map(|frame| frame.into_parts().principal)
            .map_err(EngineError::unreadable)
    } else {
        continuing::preparation::prepare::<Choosing>(conn, clock, &proof.origin, input)
            .await
            .map(|frame| frame.into_parts().principal)
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
    let binding = CREATE
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
    if known.variant().operation != CREATE.operation {
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
