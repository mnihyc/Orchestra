//! Native job read branches and their factual subjects, before a Job is formatted.

use super::*;

use crate::routes::environments::reading;

use crate::routes::environments::retained;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_types::Json;

pub use orch_api_base::routes::environments::jobs::subject::Choice;

pub use orch_api_base::routes::environments::jobs::subject::Selection;
/// The existing typed Job projection's identity conversion, over its actual selected source.
/// Only native node/folder placement is inherited; creator and resource association are not ancestry.
pub(crate) fn identity(row: &JobRow, subject: Option<&KeyResolvedTarget>) -> KeyResolvedTarget {
    let mut target = subject.cloned().unwrap_or(KeyResolvedTarget {
        object: KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Job),
            id: Some(row.id),
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    });
    target.object = KeyObjectRef {
        kind: KeyTargetKind::Identity(KeyIdentityKind::Job),
        id: Some(row.id),
    };
    target.credential_account = None;
    target
}
orch_api_base::__api_bind_302f5be720!();

pub(crate) fn scope_refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the job's native subject has not been admitted",
    )
}

#[derive(Clone, Copy)]
enum Gate {
    Maker,
    WorkspaceMaker,
    Resource(ResourceKind, Id, Permission),
    Thread(Id),
    Source(Id),
    Environment(Id),
    WorkspaceEnvironment(Id),
    WorkspaceProvider(Id),
    Notice(Id),
    Retained(Id),
    NodeControl(Id, NodePermission, bool),
    MeasurementCancel,
}

#[derive(Clone, Copy)]
pub(crate) struct Candidate {
    pub(crate) choice: Choice,
    gate: Gate,
}

fn resource(kind: ResourceKind, id: Id, permission: Permission) -> Candidate {
    Candidate {
        choice: Choice::object(kind.into(), id),
        gate: Gate::Resource(kind, id, permission),
    }
}

fn thread(id: Id) -> Candidate {
    Candidate {
        choice: Choice::object(ObjectKind::Thread, id),
        gate: Gate::Thread(id),
    }
}

fn provider(id: Id) -> Candidate {
    Candidate {
        choice: Choice::provider(id),
        gate: Gate::Resource(ResourceKind::Provider, id, Permission::ProviderManage),
    }
}

/// Creator custody needs no live parent Read, but its genuine current placement still
/// constrains the credential. Never erase a real place exclusion or infer Account ancestry.
fn maker_candidates(row: &JobRow, gate: Gate) -> Vec<Candidate> {
    let mut found = Vec::new();
    for namespace in [
        row.node_id.map(|id| ObjectRef {
            kind: ObjectKind::Thread,
            id,
        }),
        row.environment_id.map(|id| ObjectRef {
            kind: ObjectKind::Environment,
            id,
        }),
        row.provider_id.map(|id| ObjectRef {
            kind: ObjectKind::Provider,
            id,
        }),
    ]
    .into_iter()
    .flatten()
    {
        found.push(Candidate {
            choice: Choice {
                namespace: Some(namespace),
                ..Choice::maker()
            },
            gate,
        });
    }
    if found.is_empty() {
        found.push(Candidate {
            choice: Choice::maker(),
            gate,
        });
    }
    found
}

/// This is the original kind dispatch, extracted once before any renderer or scope decision.
fn native_candidates(row: &JobRow) -> ApiResult<Vec<Candidate>> {
    let one = |candidate| Ok(vec![candidate]);
    if let Some(message) = message_notice_candidate(row)? {
        return one(message);
    }
    let environment = || row.environment_id.ok_or_else(ApiError::not_found);
    if row.kind == orch_engine::jobs::cleanup::KIND || row.kind == orch_engine::resource_pause::KIND
    {
        return Ok(maker_candidates(row, Gate::Maker));
    }
    if row.kind == kind::DELIVERY && row.plan.get("workspace_upload").is_some() {
        return one(resource(
            ResourceKind::Environment,
            environment()?,
            Permission::FilesView,
        ));
    }
    if row.kind == orch_engine::jobs::host_delete::KIND {
        let host = serde_json::from_value(row.plan["cascade"]["host"].clone())
            .map_err(ApiError::internal)?;
        return one(resource(ResourceKind::Host, host, Permission::HostView));
    }
    if row.kind == orch_engine::jobs::measurement::KIND {
        let subject = measurement_subject(row)?;
        let (kind, permission) = match subject.kind {
            ObjectKind::Environment => (ResourceKind::Environment, Permission::FilesView),
            ObjectKind::Host => (ResourceKind::Host, Permission::HostManage),
            ObjectKind::Mount => (ResourceKind::Mount, Permission::MountUse),
            _ => return Err(ApiError::not_found()),
        };
        return one(resource(kind, subject.id, permission));
    }
    if row.kind == orch_db::queries::thread_continuations::JOB_KIND {
        return one(thread(row.node_id.ok_or_else(ApiError::not_found)?));
    }
    if kind::SOURCE.contains(&row.kind.as_str()) {
        let id = environment()?;
        return one(Candidate {
            choice: Choice::object(ObjectKind::Environment, id),
            gate: Gate::Source(id),
        });
    }
    if kind::WORKSPACE.contains(&row.kind.as_str()) {
        let mut found = Vec::new();
        if row.plan.get("workspace_export").and_then(Json::as_bool) == Some(true) {
            found.extend(maker_candidates(row, Gate::WorkspaceMaker));
        }
        if let Some(id) = row.node_id {
            found.push(thread(id));
        }
        if let Some(id) = row.environment_id {
            found.push(Candidate {
                choice: Choice::object(ObjectKind::Environment, id),
                gate: Gate::WorkspaceEnvironment(id),
            });
        }
        if let Some(id) = row.provider_id {
            found.push(Candidate {
                gate: Gate::WorkspaceProvider(id),
                ..provider(id)
            });
        }
        return Ok(found);
    }
    if row.kind == orch_engine::jobs::document_notices::KIND {
        let document = serde_json::from_value(row.plan["notice"]["document"].clone())
            .map_err(ApiError::internal)?;
        return one(Candidate {
            choice: Choice::object(ObjectKind::Document, document),
            gate: Gate::Notice(document),
        });
    }
    if row.kind == kind::EXPORT || row.kind == kind::IMPORT {
        return Ok(maker_candidates(row, Gate::Maker));
    }
    if matches!(
        row.kind.as_str(),
        rows::retained::RESTORE | rows::retained::PURGE
    ) {
        let id = environment()?;
        return one(Candidate {
            choice: Choice::object(ObjectKind::Environment, id),
            gate: Gate::Retained(id),
        });
    }
    let mut found = Vec::new();
    if let Some(id) = row.environment_id {
        found.push(Candidate {
            choice: Choice::object(ObjectKind::Environment, id),
            gate: Gate::Environment(id),
        });
    }
    if let Some(id) = row.node_id {
        found.push(thread(id));
    }
    if let Some(id) = row.provider_id {
        found.push(provider(id));
    }
    Ok(found)
}

/// An administrator notice job contains an accepted message identity/count, not document content.
/// Creator custody keeps its real org namespace; a node_id here is never a thread alias.
fn message_notice_candidate(row: &JobRow) -> ApiResult<Option<Candidate>> {
    if row.kind != orch_engine::jobs::document_notices::KIND
        || row.plan.get("admin_message").is_none()
    {
        return Ok(None);
    }
    let _message: Id =
        serde_json::from_value(row.plan["admin_message"].clone()).map_err(ApiError::internal)?;
    Ok(Some(Candidate {
        choice: Choice {
            namespace: row.node_id.map(|id| ObjectRef {
                kind: ObjectKind::Org,
                id,
            }),
            ..Choice::maker()
        },
        gate: Gate::Maker,
    }))
}

/// Native node View accepts the stored node's real kind. The column's historical
/// "thread" name is not permission to relabel a Topic/Project job as a Thread.
pub(crate) async fn candidates(conn: &mut Conn, row: &JobRow) -> ApiResult<Vec<Candidate>> {
    normalize(conn, native_candidates(row)?).await
}

async fn normalize(conn: &mut Conn, mut found: Vec<Candidate>) -> ApiResult<Vec<Candidate>> {
    let ids = found
        .iter()
        .filter_map(|candidate| {
            candidate
                .choice
                .namespace
                .filter(|object| object.kind == ObjectKind::Thread)
                .map(|object| object.id)
        })
        .collect::<Vec<_>>();
    if !ids.is_empty() {
        let graph = orch_authz::engine::Graph::load(conn, &ids)
            .await
            .map_err(authorization::restoration_error)?;
        for candidate in &mut found {
            if let Some(namespace) = candidate
                .choice
                .namespace
                .filter(|object| object.kind == ObjectKind::Thread)
            {
                let id = namespace.id;
                let node = graph.get(id).ok_or_else(ApiError::not_found)?;
                let actual = ObjectRef {
                    kind: ObjectKind::from(node.kind),
                    id,
                };
                candidate.choice.namespace = Some(actual);
                if candidate.choice.subject == Some(namespace) {
                    candidate.choice.subject = Some(actual);
                }
            }
        }
    }
    Ok(found)
}

/// Give the already selected native branch its actual typed node namespace.
pub(super) async fn actual_choice(conn: &mut Conn, choice: Choice) -> ApiResult<Choice> {
    normalize(
        conn,
        vec![Candidate {
            choice,
            gate: Gate::Maker,
        }],
    )
    .await?
    .into_iter()
    .next()
    .map(|candidate| candidate.choice)
    .ok_or_else(ApiError::not_found)
}

enum Decision {
    Allowed(bool),
    Skipped,
    Denied(ApiError),
    Fatal(ApiError),
}

async fn decide(
    conn: &mut Conn,
    principal: &Principal,
    row: &JobRow,
    candidate: Candidate,
) -> Decision {
    let result: ApiResult<bool> = match candidate.gate {
        Gate::Maker => makers_own(principal, row).map(|()| false),
        Gate::WorkspaceMaker => {
            if row.created_by == Some(principal.user()) {
                return Decision::Allowed(false);
            }
            return Decision::Skipped;
        }
        Gate::Resource(kind, id, permission) => authorization::require_resource(
            conn,
            principal,
            kind,
            id,
            permission,
            Policy::default(),
        )
        .await
        .map(|()| false),
        Gate::Thread(id) => {
            authorization::require(conn, principal, id, VIEW, Act::Use, Policy::default())
                .await
                .map(|()| false)
        }
        Gate::Source(environment) => {
            if authorization::require_resource(
                conn,
                principal,
                ResourceKind::Environment,
                environment,
                Permission::FilesView,
                Policy::default(),
            )
            .await
            .is_ok()
            {
                return Decision::Allowed(false);
            }
            reading::decide(conn, principal, environment)
                .await
                .map(|_| true)
        }
        Gate::Environment(environment) => match reading::decide(conn, principal, environment).await
        {
            Ok(reading::Reading::Whole) => Ok(false),
            Ok(reading::Reading::ThroughAThread) => {
                let open = match rows::open_of_environment(conn, environment).await {
                    Ok(open) => open,
                    Err(error) => return Decision::Fatal(error.into()),
                };
                if open.first().is_some_and(|current| current.id == row.id) {
                    Ok(true)
                } else {
                    return Decision::Fatal(ApiError::not_found());
                }
            }
            Err(error) => Err(error),
        },
        Gate::WorkspaceEnvironment(environment) => {
            let object = ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            };
            match orch_authz::resources::effective(conn, principal, object, Policy::default()).await
            {
                Ok(effective)
                    if effective.owner || effective.administers || !effective.held.is_empty() =>
                {
                    Ok(false)
                }
                Ok(_) => return Decision::Skipped,
                Err(error) => return Decision::Fatal(ApiError::internal(error)),
            }
        }
        Gate::WorkspaceProvider(provider) => {
            if authorization::require_resource(
                conn,
                principal,
                ResourceKind::Provider,
                provider,
                Permission::ProviderManage,
                Policy::default(),
            )
            .await
            .is_ok()
            {
                Ok(false)
            } else {
                return Decision::Skipped;
            }
        }
        Gate::Notice(document) => {
            match authorization::require_document(
                conn,
                principal,
                document,
                Permission::DocView,
                Policy::default(),
            )
            .await
            {
                Ok(()) => makers_own(principal, row).map(|()| false),
                Err(error) => Err(error),
            }
        }
        Gate::Retained(environment) => retained::may_read(conn, principal, environment)
            .await
            .map(|_| false),
        Gate::NodeControl(id, permission, view) => {
            if view
                && let Err(error) =
                    authorization::require(conn, principal, id, VIEW, Act::Use, Policy::default())
                        .await
            {
                return Decision::Denied(error);
            }
            authorization::require(
                conn,
                principal,
                id,
                PermissionName::OfNode(permission),
                Act::Use,
                Policy::default(),
            )
            .await
            .map(|()| false)
        }
        Gate::MeasurementCancel => measurement_access(conn, principal, row, true)
            .await
            .map(|()| false),
    };
    match result {
        Ok(cut) => Decision::Allowed(cut),
        Err(error) => Decision::Denied(error),
    }
}

/// Default selection preserves the old native first-success/error order. A scoped caller
/// may choose another genuine alternative, while its separately captured native cut caps output.
pub(crate) async fn read(
    conn: &mut Conn,
    principal: &Principal,
    row: &JobRow,
    selection: Option<&dyn Selection>,
) -> ApiResult<Choice> {
    let candidates = if selection.is_some() {
        candidates(conn, row).await?
    } else {
        native_candidates(row)?
    };
    choose(conn, principal, row, selection, candidates).await
}
orch_api_base::__api_bind_0f6e4906bc!();

async fn choose(
    conn: &mut Conn,
    principal: &Principal,
    row: &JobRow,
    selection: Option<&dyn Selection>,
    candidates: Vec<Candidate>,
) -> ApiResult<Choice> {
    let mut first = None;
    for candidate in candidates {
        if let Some(selection) = selection
            && !selection.accepts(conn, row, candidate.choice).await?
        {
            continue;
        }
        match decide(conn, principal, row, candidate).await {
            Decision::Allowed(cut) => {
                return Ok(Choice {
                    cut,
                    ..candidate.choice
                });
            }
            Decision::Skipped => {}
            Decision::Denied(error) => first = first.or(Some(error)),
            Decision::Fatal(error) => return Err(error),
        }
    }
    Err(first.unwrap_or_else(|| {
        if selection.is_some() {
            scope_refused()
        } else {
            ApiError::not_found()
        }
    }))
}

fn node_control(id: Id, permission: NodePermission, view: bool) -> Candidate {
    Candidate {
        choice: Choice::object(ObjectKind::Thread, id),
        gate: Gate::NodeControl(id, permission, view),
    }
}

/// Snapshot/delivery source binding and continuation's native trigger share Thread serialization.
pub(super) fn cancellation_thread(row: &JobRow) -> Option<Id> {
    ((kind::WORKSPACE.contains(&row.kind.as_str())
        && !kind::SOURCE.contains(&row.kind.as_str())
        && !(row.kind == kind::DELIVERY && row.plan.get("workspace_upload").is_some()))
        || row.kind == orch_db::queries::thread_continuations::JOB_KIND)
        .then_some(row.node_id)
        .flatten()
}

async fn native_cancellation_candidates(
    conn: &mut Conn,
    row: &JobRow,
) -> ApiResult<Vec<Candidate>> {
    let one = |candidate| Ok(vec![candidate]);
    if let Some(message) = message_notice_candidate(row)? {
        return one(message);
    }
    let environment = || row.environment_id.ok_or_else(ApiError::not_found);
    if row.kind == orch_engine::jobs::cleanup::KIND || row.kind == orch_engine::resource_pause::KIND
    {
        return Ok(maker_candidates(row, Gate::Maker));
    }
    if row.kind == kind::DELIVERY && row.plan.get("workspace_upload").is_some() {
        return one(resource(
            ResourceKind::Environment,
            environment()?,
            Permission::FilesManage,
        ));
    }
    if row.kind == orch_engine::jobs::host_delete::KIND {
        let host = serde_json::from_value(row.plan["cascade"]["host"].clone())
            .map_err(ApiError::internal)?;
        return one(resource(ResourceKind::Host, host, Permission::HostManage));
    }
    if row.kind == orch_engine::jobs::measurement::KIND {
        return one(Candidate {
            choice: Choice::object(measurement_subject(row)?.kind, measurement_subject(row)?.id),
            gate: Gate::MeasurementCancel,
        });
    }
    if row.kind == orch_db::queries::thread_continuations::JOB_KIND {
        return one(node_control(
            row.node_id.ok_or_else(ApiError::not_found)?,
            NodePermission::Interact,
            true,
        ));
    }
    if kind::SOURCE.contains(&row.kind.as_str()) {
        return one(resource(
            ResourceKind::Environment,
            environment()?,
            if row.kind == kind::WORKSPACE_PREPARE {
                Permission::EnvironmentManage
            } else {
                Permission::FilesManage
            },
        ));
    }
    if kind::WORKSPACE.contains(&row.kind.as_str()) {
        let mut found = Vec::new();
        if row.plan.get("workspace_export").and_then(Json::as_bool) == Some(true) {
            found.extend(maker_candidates(row, Gate::WorkspaceMaker));
        }
        if let Some(thread) = row.node_id
            && let Some(environment) =
                orch_db::queries::environments::workspace::thread_place(conn, thread)
                    .await?
                    .and_then(|place| place.environment_id)
        {
            found.push(resource(
                ResourceKind::Environment,
                environment,
                Permission::FilesManage,
            ));
        }
        return Ok(found);
    }
    if row.kind == kind::EXPORT
        || row.kind == kind::IMPORT
        || row.kind == orch_engine::jobs::document_notices::KIND
    {
        return Ok(maker_candidates(row, Gate::Maker));
    }
    if row.kind == "thread_fork" {
        let mut found = maker_candidates(row, Gate::WorkspaceMaker);
        if let Some(thread) = row.node_id {
            found.push(node_control(thread, NodePermission::Manage, false));
        }
        if let Some(environment) = row.environment_id {
            found.push(resource(
                ResourceKind::Environment,
                environment,
                Permission::EnvironmentManage,
            ));
        }
        return Ok(found);
    }
    if let Some(environment) = row.environment_id {
        return one(resource(
            ResourceKind::Environment,
            environment,
            Permission::EnvironmentManage,
        ));
    }
    if let Some(id) = row.provider_id {
        return one(provider(id));
    }
    if let Some(thread) = row.node_id {
        return one(node_control(thread, NodePermission::Interact, false));
    }
    Ok(Vec::new())
}

pub(super) async fn cancellation_candidates(
    conn: &mut Conn,
    row: &JobRow,
) -> ApiResult<Vec<Candidate>> {
    let found = native_cancellation_candidates(conn, row).await?;
    normalize(conn, found).await
}

/// The original cancellation predicates and first-success order, with optional factual scope selection.
pub(super) async fn cancel(
    conn: &mut Conn,
    principal: &Principal,
    row: &JobRow,
    selection: Option<&dyn Selection>,
) -> ApiResult<Choice> {
    let candidates = if selection.is_some() {
        cancellation_candidates(conn, row).await?
    } else {
        native_cancellation_candidates(conn, row).await?
    };
    choose(conn, principal, row, selection, candidates).await
}
