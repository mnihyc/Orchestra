//! Actual native source facts precede Skill creation preparation; this is not a second registration.
use super::*;
use crate::key_operations::{native, targets};
use orch_access_contract::{
    key_scope::{KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    operation_sources::Declaration,
    skill_sources::{self, Source},
};
use orch_authz::Caller;

/// Immutable native intent selected by the same typed body as its ordinary producer.
pub(super) struct Selected {
    home: Id,
    source: SkillSource,
}
impl Selected {
    pub(super) fn require_same_input(&self, body: &api::SkillCreateBody) -> ApiResult<()> {
        if self.home != body.home_id || self.source != body.from {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the native Skill source changed",
            ));
        }
        Ok(())
    }
}
pub(super) fn declaration(source: &SkillSource) -> &'static Declaration<Source> {
    match source {
        SkillSource::New { .. } => &skill_sources::NEW,
        SkillSource::Upload { .. } => &skill_sources::UPLOAD_ROW,
        SkillSource::Environment { .. } => &skill_sources::ENVIRONMENT_ROW,
        SkillSource::Git { .. } => &skill_sources::GIT,
        SkillSource::Url { .. } => &skill_sources::URL,
    }
}
/// Factual role discovery only. The canonical facet supplies each action/business family;
/// native making/source checks remain separate and run before limits, bytes or setup effects.
pub(super) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: &api::SkillCreateBody,
) -> ApiResult<(Selected, Principal, crate::registry::OperationPlan)> {
    let row = declaration(&body.from);
    let mut facts = vec![native::Facts::one(
        Source::Home,
        KeyTargetKind::Object(ObjectKind::Skill),
        targets::node(conn, body.home_id).await?,
    )];
    match &body.from {
        SkillSource::New { .. } => {}
        SkillSource::Upload { .. } => facts.push(native::Facts::one(
            Source::Upload,
            KeyTargetKind::Identity(KeyIdentityKind::Upload),
            targets::account(conn, caller.user()).await?,
        )),
        SkillSource::Environment { environment_id, .. } => facts.push(native::Facts::one(
            Source::Environment,
            KeyTargetKind::Object(ObjectKind::Environment),
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: *environment_id,
                },
            )
            .await?,
        )),
        SkillSource::Git { host_id, .. } | SkillSource::Url { host_id, .. } => {
            facts.push(native::Facts::one(
                Source::Host,
                KeyTargetKind::Object(ObjectKind::Host),
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: *host_id,
                    },
                )
                .await?,
            ))
        }
    }
    let plan = native::bind_required(ctx, row, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    may_make_at(conn, &principal, body.home_id).await?;
    may_read_directory(conn, &principal, &body.from).await?;
    if let SkillSource::Upload { blob, .. } = &body.from
        && !orch_db::queries::uploads::owns(conn, principal.user(), blob).await?
    {
        return Err(invalid(
            "from.blob",
            "this caller has no completed upload of that archive",
        ));
    }
    Ok((
        Selected {
            home: body.home_id,
            source: body.from.clone(),
        },
        principal,
        plan,
    ))
}
