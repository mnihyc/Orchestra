//! Native recipe creation, real driver-administration intent and completed singleton content.

use super::*;
use crate::key_operations::native::{self, effects};
use crate::middleware::transaction_guard;
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::{creation_home, projection::single};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::resources::{Resource, TemplateCreateReceipt, TemplateCreateResponse};
use orch_api_contract::template_sources::{self as sources, Source};
use orch_authz::{
    Caller,
    ownership::{self, NewAccess},
};
use orch_db::queries::resources::routes::NewTemplate;
use orch_types::Time;

const TEMPLATE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Template);
const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: Some(sources::GROUPS),
        nodes: [
            (ObjectKind::Org, sources::NODES[0]),
            (ObjectKind::Project, sources::NODES[1]),
            (ObjectKind::Topic, sources::NODES[2]),
            (ObjectKind::Thread, sources::NODES[3]),
        ],
        containers: None,
    },
    sources: sources::REFERENCES,
};
pub(super) struct Templates;
pub(super) struct Proposal {
    row: TemplateRow,
    access: NewAccess,
    restricted: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Created {
    id: Id,
    home: Option<Id>,
    restricted: bool,
    first_owner: Option<Id>,
}
pub(super) struct Subject {
    created: Created,
    row: TemplateRow,
}

fn administration(restricted: bool) -> native::Facts<Source> {
    native::Facts::collection(
        Source::DriverAdministration,
        if restricted {
            vec![(TEMPLATE, vec![crate::key_operations::targets::system()])]
        } else {
            Vec::new()
        },
    )
}
async fn known_facts(
    conn: &mut Conn,
    home: Option<Id>,
    restricted: bool,
) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        native::Facts::one(
            Source::Home,
            TEMPLATE,
            creation_home::target(conn, home).await?,
        ),
        administration(restricted),
    ])
}
impl effects::Effect for Templates {
    type Source = Source;
    type Query = ();
    type Body = api::TemplateCreateBody;
    type Value = Proposal;
    type Output = Created;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let name = clean_name("name", &body.name)?;
            let driver = clean_driver("driver", &body.driver)?;
            clean_spec("spec", &body.spec)?;
            let kind = body.kind.unwrap_or(UnitKind::Environment);
            let restricted = restricted_driver(conn, &driver).await?;
            let initial = known_facts(conn, body.home_id, restricted).await?;
            let known = native::bind_known(ctx, &sources::CREATE, &initial)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            creation_authority(conn, &principal, body.home_id, &driver).await?;
            creation_home::lock(conn, body.home_id).await?;
            let restricted = restricted_driver(conn, &driver).await?;
            let facts = known_facts(conn, body.home_id, restricted).await?;
            let known = native::bind_required(ctx, &sources::CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let home = creation_authority(conn, &principal, body.home_id, &driver).await?;
            validate_saved(conn, &driver, unit_word(kind), &body.spec).await?;
            let row = TemplateRow {
                id: orch_types::new_id(),
                org_id: home.org,
                home_id: body.home_id,
                owner_id: Some(principal.user()),
                name,
                description: body.description.unwrap_or_default().trim().to_owned(),
                driver,
                kind: unit_word(kind).to_owned(),
                spec: body.spec,
                enabled: true,
                revision: 1,
                units: 0,
            };
            Ok((
                Proposal {
                    row,
                    restricted,
                    access: match body.access {
                        None | Some(NewResourceAccess::Defaults) => NewAccess::Defaults,
                        Some(NewResourceAccess::OnlyMe) => NewAccess::OnlyMe,
                        Some(NewResourceAccess::Home) => NewAccess::Home,
                    },
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            creation_authority(conn, principal, value.row.home_id, &value.row.driver).await?;
            validate_saved(conn, &value.row.driver, &value.row.kind, &value.row.spec).await
        })
    }
}
async fn insert<F>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Proposal,
    before: F,
) -> ApiResult<Created>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    before()?;
    let made = queries::insert_template(
        conn,
        &NewTemplate {
            id: value.row.id,
            name: &value.row.name,
            description: &value.row.description,
            owner_id: principal.user(),
            home_id: value.row.home_id,
            kind: &value.row.kind,
            driver: &value.row.driver,
            spec: &value.row.spec,
        },
        ctx.now,
    )
    .await?;
    if !made {
        return Err(ApiError::not_found());
    }
    ownership::start_resource(
        conn,
        ObjectRef {
            kind: ObjectKind::Template,
            id: value.row.id,
        },
        principal.user(),
        value.access,
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    record(ctx,conn,orch_db::audit::action!(AuditAction::TemplateCreated),value.row.id,
        Home{node:value.row.home_id,org:value.row.org_id},
        &format!("{} added the template {}",actor_name(ctx)?,value.row.name),
        json!({"template":value.row.id,"name":value.row.name,"driver":value.row.driver,"kind":value.row.kind})).await?;
    let first_owner: Option<Id> =
        sqlx::query_scalar("SELECT owner_id FROM env_templates WHERE id=$1")
            .bind(value.row.id)
            .fetch_one(conn)
            .await
            .map_err(DbError::from)?;
    Ok(Created {
        id: value.row.id,
        home: value.row.home_id,
        restricted: value.restricted,
        first_owner,
    })
}
impl effects::Apply for Templates {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Created>>> {
        Box::pin(async move {
            Ok(effects::Outcome::Applied(
                insert(ctx, conn, principal, value, move || before.check()).await?,
            ))
        })
    }
}
impl native::Stored for Templates {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::CREATE];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let created = *ctx.retained_source::<Created>()?;
            let row = queries::templates(conn, &[created.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let home = match created.home {
                Some(id) => crate::key_operations::targets::node_identity(conn, id).await?,
                None => crate::key_operations::targets::system(),
            };
            let target = single::subject(
                conn,
                ObjectRef {
                    kind: ObjectKind::Template,
                    id: created.id,
                },
            )
            .await?;
            Ok((
                Subject { created, row },
                vec![
                    native::Facts::one(Source::Home, TEMPLATE, home),
                    administration(created.restricted),
                    native::Facts::one(Source::Created, TEMPLATE, target),
                ],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            creation_authority(conn, principal, value.created.home, &value.row.driver).await?;
            Ok(())
        })
    }
}
impl native::resource_reply::StoredResource for Templates {
    const READ_SOURCE: Source = Source::Created;
    const ROLES: &'static single::Roles = &ROLES;
    fn object(value: &Subject) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Template,
            id: value.created.id,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match super::require_read(conn, principal, value.created.id).await {
                Ok(()) => return Ok(()),
                Err(error)
                    if native::modern(principal.caller())
                        || !matches!(error.problem.status, 403 | 404) =>
                {
                    return Err(error);
                }
                Err(_) => {}
            }
            // Preserve this request's generated legacy result through its original native gate.
            creation_authority(conn, principal, value.created.home, &value.row.driver).await?;
            if value.row.home_id != value.created.home
                || value.row.owner_id != value.created.first_owner
            {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.created == fresh.created
                && opened.row.home_id == fresh.row.home_id
                && opened.row.kind == fresh.row.kind
                && opened.row.driver == fresh.row.driver)
        })
    }
}
fn receipt(id: Id) -> TemplateCreateResponse {
    TemplateCreateResponse::Receipt(TemplateCreateReceipt {
        projection: ReceiptProjection::Receipt,
        template_id: id,
    })
}
pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _body: api::TemplateCreateBody,
) -> ApiResult<TemplateCreateResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Templates>(&ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&completed.ctx, &receipt(completed.output.id))?;
    completed.ctx.retain_source(completed.output)?;
    Ok(
        match native::resource_reply::stored::<Templates>(&completed.ctx, &completed.accepted)
            .await?
        {
            Some(Resource::Template(template)) => TemplateCreateResponse::Full(Box::new(template)),
            None => receipt(completed.output.id),
            Some(_) => {
                return Err(ApiError::internal(
                    "the native created template response has another kind",
                ));
            }
        },
    )
}
