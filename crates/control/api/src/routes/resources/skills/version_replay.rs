//! Native version Read/receipt readback, with no source supplier or recording on replay.
use super::*;
use crate::key_operations::{
    native::{self, reply},
    publication, targets,
};
use crate::middleware::idempotency::Creation;
use futures::future::BoxFuture;
use orch_api_contract::{
    operation_sources::Declaration,
    skill_version_sources::{self as sources, Source},
};
use orch_authz::Caller;
use orch_db::idempotency::creation::Created;

struct Versions;
struct Accepted {
    skill: Id,
    version: Id,
    publish: bool,
    first: bool,
}
fn safe(skill: Id, version: Id) -> api::SkillVersionWriteResponse {
    api::SkillVersionWriteResponse::Receipt(api::SkillVersionWriteReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        skill_id: skill,
        version_id: version,
    })
}
fn identity(value: &api::SkillVersionWriteResponse, publish: bool) -> ApiResult<Created> {
    Ok(Created {
        kind: if publish {
            "skill_published_version"
        } else {
            "skill_version"
        }
        .into(),
        id: match value {
            api::SkillVersionWriteResponse::Full(value) => value.version.id,
            api::SkillVersionWriteResponse::Receipt(value) => value.version_id,
        },
        data: serde_json::Value::Null,
    })
}
pub(super) fn create_binding()
-> Creation<(), api::SkillVersionCreateBody, api::SkillVersionWriteResponse> {
    Creation {
        kind: "skill_version",
        replay: |ctx, (), _, created| Box::pin(replay(ctx, created, false)),
        identity: |value| identity(value, false),
    }
}
pub(super) fn publish_binding()
-> Creation<(), api::SkillDraftPublishBody, api::SkillVersionWriteResponse> {
    Creation {
        kind: "skill_published_version",
        replay: |ctx, (), _, created| Box::pin(replay(ctx, created, true)),
        identity: |value| identity(value, true),
    }
}
async fn replay(
    ctx: Ctx,
    created: Created,
    publish: bool,
) -> ApiResult<api::SkillVersionWriteResponse> {
    let skill: Id =
        serde_json::from_value(created.data["skill"].clone()).map_err(ApiError::internal)?;
    if ctx.id("id")? != skill {
        return Err(ApiError::not_found());
    }
    let receipt = safe(skill, created.id);
    publication::receipt(&ctx, &receipt)?;
    match reply::render_completed_replay::<Versions>(
        &ctx,
        ctx.user()?.id(),
        Accepted {
            skill,
            version: created.id,
            publish,
            first: false,
        },
    )
    .await
    {
        Ok(Some(value)) => Ok(api::SkillVersionWriteResponse::full(value)),
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
    version: Id,
    publish: bool,
) -> ApiResult<api::SkillVersionWriteResponse> {
    let receipt = safe(skill, version);
    publication::receipt(ctx, &receipt)?;
    match reply::render::<Versions>(
        ctx,
        accepted,
        Accepted {
            skill,
            version,
            publish,
            first: true,
        },
    )
    .await
    {
        Ok(Some(value)) => Ok(api::SkillVersionWriteResponse::full(value)),
        Ok(None) => Ok(receipt),
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}
impl reply::Projection for Versions {
    type Source = Source;
    type Input = Accepted;
    type Subject = VersionRow;
    type Value = HashMap<Id, orch_api_contract::users::UserRef>;
    type Reply = SkillVersion;
    const READ_SOURCE: Source = Source::Skill;
    fn declaration(input: &Accepted) -> &'static Declaration<Source> {
        if input.publish {
            &sources::PUBLISH
        } else {
            &sources::CREATE
        }
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(VersionRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = version_of(conn, input.skill, input.version).await?;
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Skill,
                    id: input.skill,
                },
            )
            .await?;
            Ok((
                row,
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
        input: &'a Accepted,
        _: &'a VersionRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Skill,
            input.skill,
            if input.first && !native::modern(principal.caller()) {
                Permission::SkillManage
            } else {
                Permission::SkillView
            },
        ))
    }
    fn prepare<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        row: &'a VersionRow,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            Ok((
                refusals::user_refs(conn, &row.created_by.into_iter().collect::<Vec<_>>()).await?,
                Vec::new(),
            ))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<SkillVersion>> {
        Box::pin(async move {
            Ok(SkillVersion {
                version: version_info(&value.subject, &value.value)?,
                files: files_of(&value.subject)?,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        current: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == current.subject && old.value == current.value) })
    }
}
