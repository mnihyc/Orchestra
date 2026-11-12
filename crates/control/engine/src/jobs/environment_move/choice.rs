//! The native hosted-move selection, before a count, fingerprint or workspace RPC can start.
use super::*;

/// Actual immutable operands selected by the existing hosted move planner.
#[derive(Debug, Clone)]
pub struct MoveChoice {
    /// The real source Environment row, including its current home and generation.
    pub state: envs::EnvironmentState,
    /// Its exact current runtime, never another unit named by a request.
    pub old: envs::RuntimeFull,
    /// The native chosen destination, not all candidate places.
    pub destination: crate::places::HostPlace,
    /// The native explicit/default workspace selection.
    pub selection: Selection,
    /// Installed-file source retained by the source runtime.
    pub kept_image: Option<orch_driver_contract::kept::KeptImage>,
    /// The real known installed-file transfer size, as the native planner reports it.
    pub image_bytes: i64,
}

/// Run the same destination selector used by the ordinary writer, without any measurement IO.
/// `common` is the native recreation recipe, whose source and actor are checked again here.
pub async fn select_destination(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    common: &Recipe,
    request: &EnvironmentMovePreflightBody,
    now: orch_types::Time,
) -> Result<MoveChoice, UpdateError> {
    if common.target != Target::Environment(environment)
        || common.by != principal.user()
        || common.reason != UpdatePreflightReason::Recreate
        || !updates::may_manage(conn, principal, Target::Environment(environment)).await?
    {
        return Err(UpdateError::Denied);
    }
    let state = envs::environment(conn, environment)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or(UpdateError::NotFound)?;
    if request
        .template_id
        .is_some_and(|template| Some(template) != state.template_id)
    {
        return Err(UpdateError::Invalid("A move keeps the current unit template. Change its template through an update before moving it.".into()));
    }
    let selection = move_selection(
        request.files.as_ref(),
        state.facts.get("index").and_then(Json::as_str) == Some("live"),
    )?;
    let old = envs::runtime(conn, common.old)
        .await?
        .ok_or(UpdateError::NotFound)?;
    if old.environment_id != Some(environment)
        || old.provider_id.is_some()
        || envs::live_runtime(conn, environment)
            .await?
            .is_none_or(|current| current.id != old.id)
    {
        return Err(UpdateError::NotFound);
    }

    let kept_image = old
        .kept_image
        .as_ref()
        .map(|image| serde_json::from_value::<orch_driver_contract::kept::KeptImage>(image.clone()))
        .transpose()
        .map_err(EngineError::from)?;
    let image_bytes = kept_image
        .as_ref()
        .map_or(0, |image| image.total_bytes.max(image.bytes));
    let mut template = hosts::template(conn, common.template)
        .await?
        .ok_or(UpdateError::NotFound)?;
    template.spec = common.definition.clone();
    template.driver.clone_from(&old.driver);
    let mut wanted =
        crate::places::wanted(conn, state.home_node_id, template, &state.config).await?;
    // A move may deliberately detach mounts absent on another host; its plan says which.
    wanted.mounts.clear();
    let places = crate::places::host_places_for_environment(
        conn,
        principal,
        &crate::places::Ask {
            home: state.home_node_id,
            wanted: &wanted,
            current: Some(old.host_id),
            minutes_idle: crate::places::IDLE_MINUTES_DEFAULT,
            now: now.get(),
        },
        environment,
    )
    .await?;
    let other: Vec<_> = places
        .into_iter()
        .filter(|p| p.host.id != old.host_id)
        .collect();
    let destination = crate::places::choose(&other, request.host_id)
        .map_err(|e| UpdateError::Blocked(vec![updates::blocker(e.code, e.message)]))?
        .clone();

    use orch_access_contract::{
        decisions::ObjectRef,
        permissions::{ObjectKind, Permission},
    };
    super::preparation::select_destinations(
        conn,
        &[
            (
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: destination.host.id,
                },
                Permission::HostUse,
            ),
            (
                ObjectRef {
                    kind: ObjectKind::Template,
                    id: common.template,
                },
                Permission::TemplateUse,
            ),
        ],
    )
    .await?;
    Ok(MoveChoice {
        state,
        old,
        destination,
        selection,
        kept_image,
        image_bytes,
    })
}
