//! The same native draft reader serves current readback of its skill's single draft slot.
use super::*;
use crate::key_operations::{
    native::{self, reply},
    publication, targets,
};
use crate::middleware::idempotency::ObservedCreation;
use futures::future::BoxFuture;
use orch_api_contract::{
    operation_sources::Declaration,
    skill_version_sources::{self as sources, Source},
};
use orch_authz::Caller;
use orch_db::idempotency::creation::Created;

struct Draft;
fn safe(skill: Id) -> api::SkillDraftCreateResponse {
    api::SkillDraftCreateResponse::Receipt(api::SkillDraftCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        skill_id: skill,
    })
}
pub(super) fn binding()
-> ObservedCreation<(), api::SkillDraftCreateBody, api::SkillDraftCreateResponse> {
    ObservedCreation {
        kind: "skill_draft",
        replay: |ctx, (), _, created| Box::pin(replay(ctx, created)),
    }
}
async fn replay(ctx: Ctx, created: Created) -> ApiResult<api::SkillDraftCreateResponse> {
    if ctx.id("id")? != created.id {
        return Err(ApiError::not_found());
    }
    let receipt = safe(created.id);
    publication::receipt(&ctx, &receipt)?;
    match reply::render_completed_replay::<Draft>(&ctx, ctx.user()?.id(), created.id).await {
        Ok(Some(value)) => Ok(api::SkillDraftCreateResponse::full(value)),
        Ok(None) => Ok(receipt),
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}
pub(super) async fn response(
    ctx: &Ctx,
    accepted: &Caller,
    skill: Id,
) -> ApiResult<api::SkillDraftCreateResponse> {
    let receipt = safe(skill);
    publication::receipt(ctx, &receipt)?;
    match reply::render::<Draft>(ctx, accepted, skill).await {
        Ok(Some(value)) => Ok(api::SkillDraftCreateResponse::full(value)),
        Ok(None) => Ok(receipt),
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}
impl reply::Projection for Draft {
    type Source = Source;
    type Input = Id;
    type Subject = rows::DraftInfoRow;
    type Value = (
        Vec<rows::DraftFileRow>,
        HashMap<Id, orch_api_contract::users::UserRef>,
    );
    type Reply = SkillDraft;
    const READ_SOURCE: Source = Source::Skill;
    fn declaration(_: &Id) -> &'static Declaration<Source> {
        &sources::DRAFT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        skill: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let info = rows::draft_infos(conn, &[*skill])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Skill,
                    id: *skill,
                },
            )
            .await?;
            Ok((
                info,
                vec![native::Facts::one(
                    Source::Skill,
                    target.object.kind,
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        skill: &'a Id,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // DraftGet is a manager's reader, even when a new key has the operation's Read.
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Skill,
            *skill,
            Permission::SkillManage,
        ))
    }
    fn prepare<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        skill: &'a Id,
        info: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let files = rows::draft_files(conn, *skill).await?;
            let mut ids = files.iter().map(|file| file.updated_by).collect::<Vec<_>>();
            ids.push(info.updated_by);
            let people = refusals::user_refs(conn, &ids).await?;
            Ok(((files, people), Vec::new()))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Id,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<SkillDraft>> {
        Box::pin(async move { Ok(draft_shown(&value.subject, &value.value.0, &value.value.1)) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Id,
        old: &'a reply::Prepared<Self>,
        current: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == current.subject && old.value == current.value) })
    }
}
