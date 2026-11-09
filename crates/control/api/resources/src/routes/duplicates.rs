//! Native configuration duplication. Each family keeps its own ordinary source and creation gates.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::duplicates::DuplicateBody;

use orch_api_contract::duplicates::DuplicateDestination;

use orch_api_contract::duplicates::DuplicateKind;

use orch_api_contract::duplicates::DuplicateReceipt;

use orch_types::Id;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use orch_api_contract::duplicate_sources::Source;

use orch_db::DbError;

pub use orch_api_base::routes::duplicates::Configuration;

pub use orch_api_base::routes::duplicates::register;

pub use orch_api_base::routes::duplicates::replay;

pub use orch_api_base::routes::duplicates::CreationIdentity;

pub use orch_api_base::routes::duplicates::creation_binding;

pub use orch_api_base::routes::duplicates::copy;
/// Registers native duplicate families with their actual factual producers.
pub fn mount(registry: &mut crate::registry::Registry) {
    destinations::mount(registry);
    registrations::mount(registry);
    register::<agents::Agents, _>(registry, orch_api_contract::duplicates::AgentDuplicate);
    agents::mount_moves(registry);
    skills::mount(registry);
    crate::routes::chats::duplicate::mount(registry);
    mcp::mount(registry);
    register::<mounts::Mounts, _>(registry, orch_api_contract::duplicates::MountDuplicate);
    mounts::mount_moves(registry);
    register::<templates::Templates, _>(registry, orch_api_contract::duplicates::TemplateDuplicate);
    templates::mount_moves(registry);
}
orch_api_base::__api_bind_fa1774d3f2!();

fn receipt(kind: DuplicateKind, id: Id) -> DuplicateReceipt {
    DuplicateReceipt {
        projection: ReceiptProjection::Receipt,
        kind,
        id,
    }
}
orch_api_base::__api_bind_a8c9718aea!();

pub(crate) fn destination(body: &DuplicateBody, current: Option<Id>) -> Option<Id> {
    match &body.destination {
        None => current,
        Some(DuplicateDestination::Node { id }) => Some(*id),
        Some(DuplicateDestination::System) => None,
    }
}
orch_api_base::__api_bind_6f65f9e908!();

pub(crate) fn current(ctx: &Ctx, caller: &orch_authz::Caller) -> ApiResult<()> {
    if !caller.key_deadline_allows(ctx.app.clock.as_ref())
        || ctx
            .publication
            .retained_deadline()?
            .is_some_and(|end| ctx.app.clock.time() >= end)
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the duplicate request has ended",
        ));
    }
    Ok(())
}
orch_api_base::__api_bind_e598983ef2!();

pub(crate) fn revision(body: &DuplicateBody, actual: i64) -> ApiResult<()> {
    if body
        .expected_revision
        .is_some_and(|revision| revision != actual)
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "the source changed before it could be duplicated",
        ));
    }
    Ok(())
}
orch_api_base::__api_bind_696be1c924!();

pub(crate) async fn copy_to(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    subject: orch_engine::copy_out::Subject,
    home: Option<Id>,
) -> ApiResult<Option<Id>> {
    copy_boundary(conn, principal, subject, home).await?;
    let org = match home {
        Some(id) => Some(crate::routes::nodes::placed(conn, id).await?.0.org),
        None => None,
    };
    Ok(org)
}
orch_api_base::__api_bind_f983cce395!();

pub(crate) async fn copy_boundary(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    subject: orch_engine::copy_out::Subject,
    home: Option<Id>,
) -> ApiResult<Option<orch_engine::copy_out::Boundary>> {
    match orch_engine::copy_out::authorize(conn, principal, subject, home).await {
        Ok(boundary) => Ok(boundary),
        Err(orch_engine::copy_out::Error::Export(refusal)) => {
            Err(crate::middleware::authorization::refusals::denied(conn, *refusal).await?)
        }
        Err(error) => Err(copy_error(error)),
    }
}
orch_api_base::__api_bind_cdacc514fb!();

pub(crate) fn subject(kind: DuplicateKind, id: Id) -> orch_engine::copy_out::Subject {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_engine::copy_out::Subject;
    let kind = match kind {
        DuplicateKind::Chat => return Subject::Chat(id),
        DuplicateKind::Provider => ObjectKind::Provider,
        DuplicateKind::Agent => ObjectKind::Agent,
        DuplicateKind::Template => ObjectKind::Template,
        DuplicateKind::McpServer => ObjectKind::McpServer,
        DuplicateKind::Mount => ObjectKind::Mount,
        DuplicateKind::Skill => ObjectKind::Skill,
        DuplicateKind::Environment => ObjectKind::Environment,
    };
    Subject::Object(ObjectRef { kind, id })
}
orch_api_base::__api_bind_29e0fce0ab!();

pub(crate) fn copy_error(error: orch_engine::copy_out::Error) -> ApiError {
    use orch_api_contract::common::ErrorCode;
    use orch_engine::copy_out::Error;
    match error {
        Error::NotFound => ApiError::not_found(),
        Error::Policy { at, locked, root } => ApiError::new(
            ErrorCode::AuthzDenied,
            "this place keeps its content inside its root organization",
        )
        .with(
            "setting",
            orch_settings_contract::registry::keys::SHARING_ALLOW_COPY_OUT,
        )
        .with("setting_at", at)
        .with("source_root", root)
        .with("setting_locked", locked)
        .with("setting_permission", "settings.manage"),
        Error::KnowledgeExport { root, permission } => ApiError::new(
            ErrorCode::AuthzDenied,
            "the knowledge root's Export row does not permit this copy",
        )
        .with("source_root", root)
        .with("required", permission.as_str())
        .with("page", "defaults")
        .with("next_step", "ask_organization_administrators"),
        Error::Export(_) | Error::ChatExport => ApiError::new(
            ErrorCode::AuthzDenied,
            "the source Export permission is required to copy into another organization",
        ),
        Error::Native(error) => ApiError::engine(error),
    }
}
orch_api_base::__api_bind_2078e648bd!();

/// A source export is part of the same native creation transaction, never its replay.
pub(crate) async fn record_copy(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    kind: DuplicateKind,
    source: Id,
    created: Id,
) -> ApiResult<()> {
    let (home, _) = orch_engine::copy_out::source_place(conn, subject(kind, created))
        .await
        .map_err(copy_error)?;
    if let Some(boundary) = orch_engine::copy_out::inspect(conn, subject(kind, source), home)
        .await
        .map_err(copy_error)?
    {
        orch_engine::copy_out::record(
            conn,
            boundary,
            orch_db::audit::Actor::User(ctx.user()?.id()),
            &crate::sign_in::origin(ctx),
            created,
            ctx.now,
        )
        .await
        .map_err(ApiError::engine)?;
    }
    Ok(())
}
orch_api_base::__api_bind_0a665b42ba!();

/// Tag definitions belong to one root and cannot become foreign references in a copy.
pub(crate) async fn copy_tags(
    conn: &mut orch_db::Conn,
    kind: orch_access_contract::permissions::ResourceKind,
    source: Id,
    home: Option<Id>,
) -> ApiResult<Vec<Id>> {
    let subject =
        orch_engine::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
            kind: kind.into(),
            id: source,
        });
    if orch_engine::copy_out::inspect(conn, subject, home)
        .await
        .map_err(copy_error)?
        .is_some()
    {
        return Ok(Vec::new());
    }
    Ok(orch_db::queries::resources::grouping::tag_ids(conn, kind, source).await?)
}
orch_api_base::__api_bind_6d7f4bf1fb!();

pub(crate) async fn origins(
    conn: &mut orch_db::Conn,
    kind: orch_access_contract::permissions::ObjectKind,
    id: Id,
    home: Option<Id>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let subject = crate::key_operations::targets::resource_identity(
        conn,
        orch_access_contract::decisions::ObjectRef { kind, id },
    )
    .await?;
    let place = match home {
        Some(id) => crate::key_operations::targets::node(conn, id).await?,
        None => crate::key_operations::targets::system(),
    };
    let business = orch_access_contract::key_scope::KeyTargetKind::Object(kind);
    Ok(vec![
        native::Facts::one(Source::Subject, business, subject),
        native::Facts::one(Source::Destination, business, place),
    ])
}

pub(crate) fn display_name(body: &DuplicateBody, source: &str) -> String {
    body.name
        .clone()
        .unwrap_or_else(|| format!("{} copy", source.chars().take(75).collect::<String>()))
}
orch_api_base::__api_bind_de7dea446f!();

/// Actual Owners-row standing, without treating management or administrative inspection as ownership.
pub(crate) async fn owns(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    object: orch_access_contract::decisions::ObjectRef,
) -> ApiResult<bool> {
    Ok(
        orch_authz::resources::effective(conn, principal, object, orch_authz::Policy::default())
            .await
            .map_err(ApiError::internal)?
            .owner,
    )
}
orch_api_base::__api_bind_0fea42175e!();

/// Open only explicitly selected native secret names, after the family has established real ownership.
/// The original writer seals them under the new owner's identity; sealed source bytes are never transplanted.
pub(crate) async fn secret_values(
    conn: &mut orch_db::Conn,
    kind: &str,
    owner: Id,
    names: &std::collections::BTreeSet<String>,
) -> ApiResult<std::collections::BTreeMap<String, String>> {
    let mut values = std::collections::BTreeMap::new();
    for name in names {
        let Some(row) = orch_db::queries::settings::secret(conn, kind, owner, name).await? else {
            continue;
        };
        let plain = crate::routes::resources::sealing::installed_master_key()?
            .open(
                &row.sealed,
                &crate::routes::resources::sealing::binding(kind, owner, name),
            )
            .map_err(|_| ApiError::internal("the source credential could not be opened"))?;
        let plain = String::from_utf8(plain)
            .map_err(|_| ApiError::internal("the source credential is not a text value"))?;
        values.insert(name.clone(), plain);
    }
    Ok(values)
}
orch_api_base::__api_bind_090d6c0668!();

#[path = "duplicates/agents.rs"]
pub mod agents;

#[path = "duplicates/destinations.rs"]
pub mod destinations;

#[path = "duplicates/mcp.rs"]
pub mod mcp;

#[path = "duplicates/mounts.rs"]
pub mod mounts;

#[path = "duplicates/moving.rs"]
pub mod moving;

#[path = "duplicates/registrations.rs"]
pub mod registrations;

#[path = "duplicates/skills.rs"]
pub mod skills;

#[path = "duplicates/templates.rs"]
pub mod templates;
