//! Global profile creation reuses the current native singleton styles and metadata cuts.

use super::*;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::idempotency::Creation;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use orch_access_contract::permissions::ObjectKind as K;

use orch_api_contract::agent_sources as profiles;

use orch_api_contract::resources::OrgAgentCreateReceipt;

use orch_api_contract::resources::OrgAgentCreateResponse;

use orch_api_contract::resources::Resource;

use orch_types::Id;
#[derive(Clone, Copy, PartialEq, Eq)]
struct Created {
    id: Id,
}

struct Subject {
    id: Id,
    revision: i64,
}

struct GlobalProfiles;

const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: Some(profiles::GROUPS),
        nodes: [
            (K::Org, profiles::NODES[0]),
            (K::Project, profiles::NODES[1]),
            (K::Topic, profiles::NODES[2]),
            (K::Thread, profiles::NODES[3]),
        ],
        containers: None,
    },
    sources: profiles::REFERENCES,
};

fn receipt(id: Id) -> OrgAgentCreateResponse {
    OrgAgentCreateResponse::Receipt(OrgAgentCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        agent_id: id,
    })
}

pub(super) fn binding() -> Creation<(), OrgAgentCreateBody, OrgAgentCreateResponse> {
    Creation {
        kind: "agent",
        replay,
        identity: |value| {
            let id = match value {
                OrgAgentCreateResponse::Full(value) => value.base.id,
                OrgAgentCreateResponse::Receipt(value) => value.agent_id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "agent".into(),
                id,
                data: Json::Null,
            })
        },
    }
}

fn replay(
    ctx: Ctx,
    (): (),
    _body: OrgAgentCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<OrgAgentCreateResponse>> {
    Box::pin(async move {
        publication::receipt(&ctx, &receipt(created.id))?;
        ctx.retain_source(Created { id: created.id })?;
        let current = ctx.user()?.id();
        match native::resource_reply::stored_completed_replay::<GlobalProfiles>(&ctx, current).await
        {
            Ok(Some(Resource::Agent(agent))) => Ok(OrgAgentCreateResponse::Full(Box::new(agent))),
            Ok(Some(_)) => Err(ApiError::internal("the profile singleton changed kind")),
            Ok(None) => Ok(receipt(created.id)),
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                Ok(receipt(created.id))
            }
            Err(error) => Err(error),
        }
    })
}

pub(super) async fn completed(
    ctx: &Ctx,
    accepted: &Caller,
    id: Id,
) -> ApiResult<OrgAgentCreateResponse> {
    publication::receipt(ctx, &receipt(id))?;
    ctx.retain_source(Created { id })?;
    match native::resource_reply::stored::<GlobalProfiles>(ctx, accepted).await? {
        Some(Resource::Agent(agent)) => Ok(OrgAgentCreateResponse::Full(Box::new(agent))),
        Some(_) => Err(ApiError::internal("the profile singleton changed kind")),
        None => Ok(receipt(id)),
    }
}

impl native::Stored for GlobalProfiles {
    type Source = profiles::Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Self::Source>] = &[profiles::SYSTEM_CREATE];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'static Declaration<Self::Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Self::Source>>)>> {
        Box::pin(async move {
            let created = ctx.retained_source::<Created>()?;
            let row = agent_rows::agents(conn, &[created.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            if row.home_id.is_some() || row.machine_id.is_some() {
                return Err(ApiError::not_found());
            }
            let family = KeyTargetKind::Object(K::Agent);
            Ok((
                Subject {
                    id: row.id,
                    revision: row.revision,
                },
                vec![
                    native::Facts::one(profiles::Source::Home, family, targets::system()),
                    native::Facts::one(profiles::Source::Created, family, targets::system()),
                ],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        p: &'a Principal,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::super::native::standing(ctx, p) })
    }
}

impl native::resource_reply::StoredResource for GlobalProfiles {
    const READ_SOURCE: profiles::Source = profiles::Source::Created;
    const ROLES: &'static single::Roles = &ROLES;
    fn object(value: &Subject) -> ObjectRef {
        ObjectRef {
            kind: K::Agent,
            id: value.id,
        }
    }
    fn require_read<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        p: &'a Principal,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::super::native::standing(ctx, p) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        old: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.id == fresh.id && old.revision == fresh.revision) })
    }
}
