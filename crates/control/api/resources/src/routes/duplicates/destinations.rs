//! One effect-free destination reader over the same native source and creation doors as the writers.

use super::*;

use crate::key_operations::targets;

use crate::registry::OptionalOperationFuture;

use futures::future::BoxFuture;

use orch_access_contract::decisions::Decision;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::NodePermission;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::PermissionName;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::duplicate_sources::COPY_DESTINATIONS;

use orch_api_contract::duplicate_sources::Source;

use orch_api_contract::duplicates::COPY_DESTINATIONS_MAX;

use orch_api_contract::duplicates::CopyDestinationCause as Cause;

use orch_api_contract::duplicates::CopyDestinationChoice as Choice;

use orch_api_contract::duplicates::CopyDestinationJudgement;

use orch_api_contract::duplicates::CopyDestinationOutcome as Outcome;

use orch_api_contract::duplicates::CopyDestinationsJudge;

use orch_api_contract::duplicates::CopyDestinationsRequest;

use orch_api_contract::duplicates::CopyDestinationsResponse;

use orch_api_contract::duplicates::CopyIntent;

use orch_api_contract::duplicates::CopySubject;

use orch_api_contract::duplicates::CopySubjectKind;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::chats;

use orch_db::queries::hierarchy;

use orch_db::queries::resources as rows;

use orch_engine::copy_out::Subject;
struct Destinations;

struct Value {
    request: CopyDestinationsRequest,
    source_home: Option<Id>,
    source_org: Option<Id>,
    reply: CopyDestinationsResponse,
}

pub(super) fn mount(registry: &mut crate::registry::Registry) {
    registry.on_snapshot_body_read_with_optional_operation(CopyDestinationsJudge, prepare, read);
}

fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: CopyDestinationsRequest,
) -> OptionalOperationFuture<'a> {
    native::selected::<Destinations>(ctx, conn, caller, body, ())
}

async fn read(
    ctx: Ctx,
    (): (),
    body: CopyDestinationsRequest,
) -> ApiResult<CopyDestinationsResponse> {
    native::selected_read::<Destinations>(ctx, body, ()).await
}

pub(super) fn source(subject: CopySubject) -> Subject {
    use CopySubjectKind as K;
    let kind = match subject.kind {
        K::Chat => return Subject::Chat(subject.id),
        K::Provider => ObjectKind::Provider,
        K::Agent => ObjectKind::Agent,
        K::Template => ObjectKind::Template,
        K::McpServer => ObjectKind::McpServer,
        K::Mount => ObjectKind::Mount,
        K::Skill => ObjectKind::Skill,
        K::Environment => ObjectKind::Environment,
        K::Thread => ObjectKind::Thread,
        K::Document => ObjectKind::Document,
        K::DocumentContainer => ObjectKind::DocumentContainer,
        K::Host => ObjectKind::Host,
        K::Machine => ObjectKind::Machine,
    };
    Subject::Object(ObjectRef {
        kind,
        id: subject.id,
    })
}

fn family(subject: CopySubject) -> KeyTargetKind {
    match source(subject) {
        Subject::Chat(_) => KeyTargetKind::Identity(KeyIdentityKind::Chat),
        Subject::Object(object) => KeyTargetKind::Object(object.kind),
    }
}

async fn source_target(
    conn: &mut Conn,
    subject: CopySubject,
) -> ApiResult<orch_access_contract::key_scope::KeyResolvedTarget> {
    match source(subject) {
        Subject::Chat(id) => targets::chat(conn, id).await,
        Subject::Object(object) if object.kind == ObjectKind::Machine => {
            targets::machine_subject(conn, object.id).await
        }
        Subject::Object(object) if object.kind == ObjectKind::McpServer => {
            let row = rows::mcp::subject(conn, object.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            targets::page_object(
                conn,
                ObjectRef {
                    kind: if row.environment_id.is_some() {
                        ObjectKind::Environment
                    } else {
                        ObjectKind::McpServer
                    },
                    id: row.environment_id.unwrap_or(object.id),
                },
            )
            .await
        }
        Subject::Object(object) => targets::page_object(conn, object).await,
    }
}

async fn facts(
    conn: &mut Conn,
    request: &CopyDestinationsRequest,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut selected = vec![source_target(conn, request.subject).await?];
    for destination in &request.destinations {
        match destination {
            DuplicateDestination::System => selected.push(targets::system()),
            DuplicateDestination::Node { id } => {
                // A missing candidate contributes a factual absence, never invented ancestry.
                if hierarchy::node(conn, *id).await?.is_some() {
                    selected.push(targets::node(conn, *id).await?);
                }
            }
        }
    }
    Ok(vec![
        native::Facts::collection(
            Source::DestinationJudgement,
            vec![(family(request.subject), selected.clone())],
        ),
        native::Facts::collection(
            Source::DestinationJudgementContent,
            vec![(family(request.subject), selected)],
        ),
    ])
}

impl native::Selected for Destinations {
    type Source = Source;
    type Query = CopyDestinationsRequest;
    type Value = Value;
    type Reply = CopyDestinationsResponse;
    const DECLARATION: &'static Declaration<Source> = &COPY_DESTINATIONS;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        request: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if request.destinations.len() > COPY_DESTINATIONS_MAX {
                return Err(ApiError::invalid(
                    "destinations",
                    "send at most one hundred destination candidates at a time",
                ));
            }
            let facts = facts(conn, request).await?;
            let plan = native::bind(ctx, &COPY_DESTINATIONS, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            readable(conn, &principal, request.subject).await?;
            let (source_home, source_org) =
                orch_engine::copy_out::source_place(conn, source(request.subject))
                    .await
                    .map_err(copy_error)?;
            let mut destinations = Vec::with_capacity(request.destinations.len());
            for destination in &request.destinations {
                destinations
                    .push(judge(ctx, conn, &principal, request, destination, source_org).await?);
            }
            let reply = CopyDestinationsResponse {
                subject: request.subject,
                intent: request.intent,
                source_owned: orch_engine::copy_out::owns(
                    conn,
                    &principal,
                    source(request.subject),
                )
                .await
                .map_err(copy_error)?,
                destinations,
            };
            Ok((
                Value {
                    request: request.clone(),
                    source_home,
                    source_org,
                    reply,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<CopyDestinationsResponse>> {
        Box::pin(async move {
            readable(conn, principal, value.request.subject).await?;
            Ok(value.reply.clone())
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        opened: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.request == fresh.request
                && opened.source_home == fresh.source_home
                && opened.source_org == fresh.source_org
                && opened.reply == fresh.reply)
        })
    }
}

fn blocked(cause: Cause, error: ApiError) -> Outcome {
    Outcome::Blocked {
        cause,
        problem: error.problem,
    }
}

fn next(choice: Choice) -> Outcome {
    Outcome::NeedsChoice { choice }
}

async fn native_decision(conn: &mut Conn, decision: Decision) -> ApiResult<()> {
    match decision {
        Decision::Allowed { .. } => Ok(()),
        Decision::Refused(refusal) => {
            Err(crate::middleware::authorization::refusals::denied(conn, refusal).await?)
        }
    }
}

async fn readable(conn: &mut Conn, principal: &Principal, subject: CopySubject) -> ApiResult<()> {
    match source(subject) {
        Subject::Chat(id) => {
            let row = chats::chat(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let members = chats::members(conn, id).await?;
            let policy = chat_policy(conn, row.home()).await?;
            if row.kind != "group"
                || orch_authz::chats::standing(conn, principal, &row, &members, policy)
                    .await
                    .map_err(ApiError::internal)?
                    .reading
                    .is_none()
            {
                return Err(ApiError::not_found());
            }
            Ok(())
        }
        Subject::Object(object) if object.kind == ObjectKind::Thread => {
            let decision = orch_authz::nodes::check(
                conn,
                principal,
                object.id,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            native_decision(conn, decision).await
        }
        Subject::Object(object) if object.kind.is_document() => {
            let decision = orch_authz::documents::check(
                conn,
                principal,
                object.id,
                Permission::DocView,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            native_decision(conn, decision).await
        }
        Subject::Object(object) if object.kind == ObjectKind::McpServer => {
            let row = rows::mcp::servers(conn, &[object.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            crate::routes::resources::mcp::require_row(conn, principal, &row, false).await
        }
        Subject::Object(object) if object.kind == ObjectKind::Machine => {
            let decision = orch_authz::resources::check_machine(
                conn,
                principal,
                object.id,
                Permission::MachineView,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            native_decision(conn, decision).await
        }
        Subject::Object(object) => {
            let kind = object.kind.resource().ok_or_else(ApiError::not_found)?;
            crate::routes::resources::require(
                conn,
                principal,
                kind,
                object.id,
                crate::routes::resources::view_permission(kind),
            )
            .await?;
            if object.kind == ObjectKind::Environment {
                crate::routes::resources::require(
                    conn,
                    principal,
                    kind,
                    object.id,
                    Permission::FilesView,
                )
                .await?;
            }
            Ok(())
        }
    }
}

async fn chat_policy(conn: &mut Conn, home: Id) -> ApiResult<orch_authz::chats::ChatPolicy> {
    use orch_settings_contract::registry::keys;
    let at = orch_settings::Place::Node(home);
    Ok(orch_authz::chats::ChatPolicy {
        enabled: orch_settings::get::<bool>(conn, at, keys::CHAT_ENABLED)
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(true),
        direct: orch_settings::get::<String>(conn, at, keys::CHAT_DIRECT)
            .await
            .map_err(ApiError::internal)?
            .is_none_or(|word| word != "off"),
    })
}

async fn judge(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    request: &CopyDestinationsRequest,
    destination: &DuplicateDestination,
    source_org: Option<Id>,
) -> ApiResult<CopyDestinationJudgement> {
    let (home, org) = match destination {
        DuplicateDestination::System => (None, None),
        DuplicateDestination::Node { id } => match hierarchy::node(conn, *id).await? {
            Some(row) => (Some(*id), Some(row.org_id)),
            None => {
                return Ok(CopyDestinationJudgement {
                    destination: destination.clone(),
                    cross_root: false,
                    outcome: blocked(Cause::NotAvailable, ApiError::not_found()),
                });
            }
        },
    };
    let cross_root = if request.intent == CopyIntent::Move && source_org == org {
        false
    } else {
        orch_engine::copy_out::material_boundary(conn, source(request.subject), home)
            .await
            .map_err(copy_error)?
            .is_some()
    };
    let outcome = evaluate(ctx, conn, principal, request, home, org, cross_root).await?;
    Ok(CopyDestinationJudgement {
        destination: destination.clone(),
        cross_root,
        outcome,
    })
}

async fn evaluate(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    request: &CopyDestinationsRequest,
    home: Option<Id>,
    org: Option<Id>,
    cross_root: bool,
) -> ApiResult<Outcome> {
    if let Some(home) = home {
        let decision = orch_authz::nodes::check_named(
            conn,
            principal,
            home,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        if !decision.is_allowed() {
            return Ok(blocked(Cause::NotAvailable, ApiError::not_found()));
        }
    }
    if matches!(
        request.subject.kind,
        CopySubjectKind::Host | CopySubjectKind::Machine
    ) && request.intent == CopyIntent::Duplicate
    {
        return Ok(blocked(
            Cause::RegistrationOnly,
            ApiError::invalid(
                "intent",
                "hosts and machines keep their registration and can only be moved",
            ),
        ));
    }
    if request.intent == CopyIntent::Move && !cross_root {
        return Ok(next(Choice::InOrgMove));
    }
    if cross_root
        && request.intent == CopyIntent::Move
        && !orch_engine::copy_out::owns(conn, principal, source(request.subject))
            .await
            .map_err(copy_error)?
    {
        return Ok(blocked(
            Cause::OwnersOnly,
            ApiError::new(
                ErrorCode::AuthzDenied,
                "only an actual owner can move this into another organization",
            ),
        ));
    }
    match orch_engine::copy_out::authorize(conn, principal, source(request.subject), home).await {
        Ok(_) => {}
        Err(orch_engine::copy_out::Error::Native(error)) => return Err(ApiError::engine(error)),
        Err(orch_engine::copy_out::Error::Export(refusal)) => {
            return Ok(blocked(
                Cause::ExportPermission,
                crate::middleware::authorization::refusals::denied(conn, *refusal).await?,
            ));
        }
        Err(error @ orch_engine::copy_out::Error::Policy { .. }) => {
            return Ok(blocked(Cause::SourcePolicy, copy_error(error)));
        }
        Err(error) => return Ok(blocked(Cause::ExportPermission, copy_error(error))),
    }
    if native::modern(principal.caller()) {
        return Ok(next(Choice::NativeOperationScope));
    }
    if request.subject.kind == CopySubjectKind::Thread
        && let Some(home) = home
        && hierarchy::node(conn, home)
            .await?
            .is_some_and(|row| row.kind != "topic")
    {
        return Ok(next(Choice::ForkPlan));
    }
    if let Err(error) = creation(ctx, conn, principal, request.subject, home, org).await {
        if error.problem.status >= 500 {
            return Err(error);
        }
        return Ok(blocked(Cause::DestinationCreation, error));
    }
    if request.intent == CopyIntent::Move {
        if matches!(
            request.subject.kind,
            CopySubjectKind::Host | CopySubjectKind::Machine
        ) {
            let kind = if request.subject.kind == CopySubjectKind::Host {
                ResourceKind::Host
            } else {
                ResourceKind::Machine
            };
            if let Err(error) =
                super::registrations::deletion_allowed(conn, kind, request.subject.id).await
            {
                return Ok(blocked(Cause::DeleteRefused, error));
            }
        }
        if request.subject.kind == CopySubjectKind::Environment {
            let row =
                orch_db::queries::environments::routes::environments(conn, &[request.subject.id])
                    .await?
                    .pop()
                    .ok_or_else(ApiError::not_found)?;
            if let Err(error) = orch_engine::environments::ending::review(conn, &row).await {
                return Ok(blocked(
                    Cause::DeleteRefused,
                    crate::routes::environments::ending_error(error),
                ));
            }
        }
        if request.subject.kind == CopySubjectKind::Template {
            let row = rows::routes::templates(conn, &[request.subject.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            if let Err(error) = crate::routes::resources::templates::deletion_allowed(ctx, &row) {
                return Ok(blocked(Cause::DeleteRefused, error));
            }
        }
        return Ok(next(Choice::DeletePreview));
    }
    Ok(match request.subject.kind {
        CopySubjectKind::Environment => next(Choice::EnvironmentPlacement),
        CopySubjectKind::Thread => next(Choice::ForkPlan),
        CopySubjectKind::DocumentContainer => next(Choice::DocumentTreePreview),
        CopySubjectKind::McpServer => {
            let row = rows::mcp::servers(conn, &[request.subject.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let retained = match row.machine_id {
                Some(machine) if row.imported => {
                    orch_engine::environment_mcp::duplicating::machine_allowed(
                        conn, principal, machine, home,
                    )
                    .await
                    .map_err(|error| match error {
                        orch_engine::operation_scope::continuing::Error::Native(error) => {
                            ApiError::engine(error)
                        }
                        _ => ApiError::new(
                            ErrorCode::AuthzDenied,
                            "choose a machine usable by the destination organization",
                        ),
                    })?
                }
                _ => !row.imported,
            };
            if row.imported && !retained {
                next(Choice::McpMachine)
            } else {
                Outcome::Allowed
            }
        }
        _ => Outcome::Allowed,
    })
}

async fn creation(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    subject: CopySubject,
    home: Option<Id>,
    org: Option<Id>,
) -> ApiResult<()> {
    use crate::routes::resources;
    use CopySubjectKind as K;
    match source(subject) {
        Subject::Chat(id) => {
            let home = home.ok_or_else(|| {
                ApiError::invalid("destination", "a group chat needs an organization home")
            })?;
            let root = org.ok_or_else(ApiError::not_found)?;
            crate::routes::chats::duplicate::copy_destination(
                ctx,
                conn,
                principal,
                id,
                root,
                home,
                "Destination check",
            )
            .await?;
            Ok(())
        }
        Subject::Object(object)
            if object.kind == ObjectKind::Thread || object.kind == ObjectKind::Environment =>
        {
            let home = home.ok_or_else(|| {
                ApiError::invalid("destination", "this object needs a hierarchy home")
            })?;
            let permission = if object.kind == ObjectKind::Thread {
                PermissionName::Exact(Permission::TopicStartThreads)
            } else {
                PermissionName::OfNode(NodePermission::Interact)
            };
            let decision = orch_authz::nodes::check_named(
                conn,
                principal,
                home,
                permission,
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            native_decision(conn, decision).await
        }
        Subject::Object(object) if object.kind.is_document() => {
            let home = home.ok_or_else(|| {
                ApiError::invalid("destination", "a document needs a hierarchy home")
            })?;
            let allowed = orch_engine::document_tree::can_create_in(
                conn,
                orch_engine::document_tree::Actor::Person(principal),
                orch_engine::document_tree::Directory::NodeDocs(home),
                principal.now(),
            )
            .await;
            if crate::routes::document_tree::result(conn, allowed).await? {
                Ok(())
            } else {
                Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the destination's native document creation rule does not permit this copy",
                ))
            }
        }
        Subject::Object(_) if subject.kind == K::Agent => {
            resources::creation_home::require(
                conn,
                principal,
                ResourceKind::Agent,
                home,
                "an agent profile needs an organization home",
                "only a system administrator creates a system profile",
            )
            .await?;
            let row = rows::routes::agents(conn, &[subject.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            if let Some(machine) = row.machine_id {
                let home = home.ok_or_else(|| {
                    ApiError::invalid(
                        "destination",
                        "a physical profile needs an organization home",
                    )
                })?;
                resources::agents::creation::duplicate_machine(
                    conn,
                    principal,
                    machine,
                    home,
                    &row.harness_kind,
                )
                .await?;
            }
            Ok(())
        }
        Subject::Object(_) if subject.kind == K::Skill => {
            resources::skills::may_make_at(
                conn,
                principal,
                home.ok_or_else(|| {
                    ApiError::invalid("destination", "a skill needs an organization or project")
                })?,
            )
            .await
        }
        Subject::Object(_) if subject.kind == K::Template => {
            let row = rows::routes::templates(conn, &[subject.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            resources::templates::creation_authority(conn, principal, home, &row.driver)
                .await
                .map(|_| ())
        }
        Subject::Object(object) => {
            let kind = object.kind.resource().ok_or_else(ApiError::not_found)?;
            resources::creation_home::require(
                conn,
                principal,
                kind,
                home,
                "this resource needs an organization home",
                "only a system administrator creates a system resource",
            )
            .await
            .map(|_| ())
        }
    }
}
