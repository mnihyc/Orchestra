//! Organization creation uses the native node writer and the shared guarded effect/response adapters.
use super::*;
use crate::key_operations::native::{effects, reply};
use crate::key_operations::{native, publication, targets};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::hierarchy_sources::{self as sources, Source};
use orch_api_contract::nodes::{Org, OrgChildCreateBody, OrgCreateBody, OrgCreateResponse};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;

pub(super) struct Root;
pub(super) struct Child;

pub(super) fn replay_binding<B: Send + 'static, const ROOT: bool>()
-> crate::middleware::idempotency::Creation<(), B, OrgCreateResponse> {
    crate::middleware::idempotency::Creation {
        kind: "org",
        replay: replay::<B, ROOT>,
        identity: replay_identity,
    }
}
fn replay_identity(
    value: &OrgCreateResponse,
) -> ApiResult<orch_db::idempotency::creation::Created> {
    let id = match value {
        OrgCreateResponse::Full(value) => value.base.id,
        OrgCreateResponse::Receipt(value) => value.node_id,
    };
    Ok(orch_db::idempotency::creation::Created {
        kind: "org".into(),
        id,
        data: serde_json::Value::Null,
    })
}
fn replay<B: Send + 'static, const ROOT: bool>(
    ctx: Ctx,
    (): (),
    _body: B,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<OrgCreateResponse>> {
    Box::pin(async move {
        let receipt = OrgCreateResponse::Receipt(super::super::creation_operation::receipt(
            created.id,
            NodeKind::Org,
        ));
        publication::receipt(&ctx, &receipt)?;
        let row = {
            let mut conn = ctx.app.db.acquire().await?;
            hierarchy::node(&mut conn, created.id).await?
        };
        let Some(row) = row else { return Ok(receipt) };
        let input = AcceptedOrganization {
            root: ROOT,
            parent: if ROOT { None } else { row.parent_id },
            id: created.id,
        };
        match reply::render_completed_replay::<Content>(&ctx, ctx.user()?.id(), input).await {
            Ok(Some(value)) => Ok(OrgCreateResponse::Full(Box::new(value))),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}
pub(super) struct PreparedOrganization {
    root: bool,
    parent: Option<Id>,
    name: String,
    slug: String,
    description: String,
    access: Option<NewNodeAccess>,
    general: bool,
    first: Id,
    invitation: Option<identity::OrgInvitation>,
    creation: Option<OrgCreation>,
}
pub(super) struct AcceptedOrganization {
    root: bool,
    parent: Option<Id>,
    id: Id,
}
fn declaration(root: bool) -> &'static Declaration<Source> {
    if root {
        &sources::CREATE
    } else {
        &sources::CHILD
    }
}
async fn destination(
    _ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    parent: Option<Id>,
) -> ApiResult<native::Facts<Source>> {
    let target = match parent {
        Some(id) => targets::node(conn, id).await?,
        None => targets::account(conn, caller.user()).await?,
    };
    if parent.is_some() && target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(
        Source::Destination,
        KeyTargetKind::Object(ObjectKind::Org),
        target,
    ))
}
fn creation_refused(creation: OrgCreation, why: &str) -> ApiError {
    ApiError::new(ErrorCode::OrgCreationDisabled, why.to_owned()).with("org_creation", creation)
}
async fn prepare_root(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: OrgCreateBody,
) -> ApiResult<(PreparedOrganization, Vec<native::Facts<Source>>)> {
    let name = clean_name("name", &body.name)?;
    let slug = clean_slug("slug", &body.slug, ROOT_SLUG_MIN_CHARS)?;
    let creation = crate::sign_in::system_policy(conn).await?.org_creation;
    let actor = ctx.user()?;
    // Invitation writers serialize here without upgrading the identity guard's KEY SHARE lock.
    // After a competing maker commits, this locking read sees the invitation it consumed.
    let row: (Option<Id>, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT org_invited_by,org_invited_at FROM users WHERE id=$1 FOR NO KEY UPDATE",
    )
    .bind(caller.user())
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let invitation = match row {
        (Some(given_by), Some(given_at)) => Some(identity::OrgInvitation { given_by, given_at }),
        _ => None,
    };
    if actor.user.kind != "human" {
        return Err(creation_refused(
            creation,
            "a service identity makes no team org",
        ));
    }
    let facts = vec![destination(ctx, conn, caller, None).await?];
    let known = native::bind_required(ctx, &sources::CREATE, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    let preliminary = PreparedOrganization {
        root: true,
        parent: None,
        name: name.clone(),
        slug: slug.clone(),
        description: String::new(),
        access: None,
        general: true,
        first: caller.user(),
        invitation,
        creation: Some(creation),
    };
    Box::pin(require(ctx, conn, &principal, &preliminary)).await?;
    slug_free(conn, None, &slug).await?;
    let first = match orch_authz::ownership::owner_of_new(conn, caller.user(), None).await {
        Ok(first) => first,
        Err(error) => {
            let error = refusals::problem(conn, error).await;
            return Err(if native::modern(caller) {
                authorization::problem_without_content(error)
            } else {
                error
            });
        }
    };
    Ok((
        PreparedOrganization {
            root: true,
            parent: None,
            name,
            slug,
            description: String::new(),
            access: None,
            general: true,
            first,
            invitation,
            creation: Some(creation),
        },
        facts,
    ))
}
async fn prepare_child(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: OrgChildCreateBody,
) -> ApiResult<(PreparedOrganization, Vec<native::Facts<Source>>)> {
    let parent = ctx.id("id")?;
    let name = clean_name("name", &body.name)?;
    let slug = clean_slug("slug", &body.slug, SLUG_MIN_CHARS)?;
    let description = clean_description("description", body.description.as_deref().unwrap_or(""))?;
    hierarchy::lock_live_org(conn, parent)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (place, kind) = super::super::placed(conn, parent).await?;
    if kind != NodeKind::Org {
        return Err(ApiError::not_found());
    }
    if place.org != parent {
        hierarchy::lock_live_org(conn, place.org)
            .await?
            .ok_or_else(ApiError::not_found)?;
    }
    let facts = vec![destination(ctx, conn, caller, Some(parent)).await?];
    let known = native::bind_required(ctx, &sources::CHILD, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    authorization::require_on_path_in(ctx, conn, &principal, "org.manage").await?;
    slug_free_for_operation(ctx, conn, caller, parent, &slug).await?;
    let rows = hierarchy::with_ancestors(conn, &[parent]).await?;
    let above = rows
        .iter()
        .find(|row| row.id == parent)
        .map(|row| {
            row.path
                .iter()
                .filter(|id| rows.iter().any(|row| row.id == **id && row.kind == "org"))
                .count()
        })
        .unwrap_or_default();
    if orgs::limits(conn, place.org)
        .await
        .map_err(ApiError::internal)?
        .is_some_and(|limits| limits.flat)
    {
        let error = ApiError::new(ErrorCode::OrgFlat, "this tree holds no org inside it");
        return Err(if !native::modern(caller) {
            error.with("org", refusals::named_node(conn, place.org).await?)
        } else {
            error
        });
    }
    let limit = ctx.app.config.limits.org_depth;
    if !orgs::fits_depth(above + 1, limit) {
        return Err(ApiError::new(
            ErrorCode::OrgTooDeep,
            format!("orgs may lie in one another {limit} deep at most"),
        )
        .with("limit", limit));
    }
    let first = match orch_authz::ownership::owner_of_new(conn, caller.user(), Some(parent)).await {
        Ok(first) => first,
        Err(error) => {
            let error = refusals::problem(conn, error).await;
            return Err(if native::modern(caller) {
                authorization::problem_without_content(error)
            } else {
                error
            });
        }
    };
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
        .bind(first)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    Ok((
        PreparedOrganization {
            root: false,
            parent: Some(parent),
            name,
            slug,
            description,
            access: body.access,
            general: body.general == Some(true),
            first,
            invitation: None,
            creation: None,
        },
        facts,
    ))
}
async fn slug_free_for_operation(
    _ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    parent: Id,
    slug: &str,
) -> ApiResult<()> {
    if !native::modern(caller) {
        return slug_free(conn, Some(parent), slug).await;
    }
    // The native collision is the same; an existing sibling's name needs its own optional Read.
    if hierarchy::org_with_slug(conn, Some(parent), slug)
        .await?
        .is_some()
    {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            format!("the address {slug} is taken"),
        )
        .with(
            "errors",
            vec![FieldFault {
                path: "slug".into(),
                message: format!("the address {slug} is taken"),
            }],
        ));
    }
    slug_free(conn, Some(parent), slug).await
}
async fn require(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &PreparedOrganization,
) -> ApiResult<()> {
    if let Some(creation) = value.creation {
        let actor = ctx.user()?;
        if actor.user.kind != "human" {
            return Err(creation_refused(
                creation,
                "a service identity makes no team org",
            ));
        }
        let administrator = if native::modern(principal.caller()) {
            ctx.ordinary_system_admin_with(actor, principal.caller())
                .is_ok()
                && principal.has_system_admin_standing()
        } else {
            actor.is_system_admin()
        };
        match creation {
            OrgCreation::Anyone => {}
            _ if administrator => {}
            OrgCreation::Admins => {
                return Err(creation_refused(
                    creation,
                    "only a system admin makes a team org here",
                ));
            }
            OrgCreation::Invited if value.invitation.is_some() => {}
            OrgCreation::Invited => {
                return Err(creation_refused(
                    creation,
                    "a team org is made here by a system admin, or by somebody a system admin invited to make one",
                ));
            }
        }
    } else {
        authorization::require_on_path_in(ctx, conn, principal, "org.manage").await?;
    }
    Ok(())
}
macro_rules! organization {
    ($kind:ident,$body:ty,$prepare:ident) => {
        impl effects::Effect for $kind {
            type Source = Source;
            type Query = ();
            type Body = $body;
            type Value = PreparedOrganization;
            type Output = AcceptedOrganization;
            fn declaration(value: &Self::Value) -> &'static Declaration<Source> {
                declaration(value.root)
            }
            fn prepare<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                caller: &'a Caller,
                (): (),
                body: $body,
            ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
                Box::pin($prepare(ctx, conn, caller, body))
            }
            fn require<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Self::Value,
            ) -> BoxFuture<'a, ApiResult<()>> {
                Box::pin(require(ctx, conn, principal, value))
            }
        }
        impl effects::Apply for $kind {
            fn apply<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Self::Value,
                before: effects::FirstEffect<'a>,
            ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
                Box::pin(apply(ctx, conn, principal, value, before))
            }
        }
    };
}
organization!(Root, OrgCreateBody, prepare_root);
organization!(Child, OrgChildCreateBody, prepare_child);
async fn apply<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    principal: &'a Principal,
    value: &'a PreparedOrganization,
    before: effects::FirstEffect<'a>,
) -> ApiResult<effects::Outcome<AcceptedOrganization>> {
    let mut before = Some(before);
    let administrator = if native::modern(principal.caller()) {
        ctx.ordinary_system_admin_with(ctx.user()?, principal.caller())
            .is_ok()
    } else {
        ctx.user()?.is_system_admin()
    };
    let invitation = if value.creation == Some(OrgCreation::Invited) && !administrator {
        let invitation =
            identity::take_org_invitation_before(conn, principal.user(), || match before.take() {
                Some(check) => check.check(),
                None => Ok(()),
            })
            .await?;
        if invitation != value.invitation {
            return Err(creation_refused(
                OrgCreation::Invited,
                "the invitation is no longer available",
            ));
        }
        invitation
    } else {
        None
    };
    let made = Box::pin(make_with_owner(
        ctx,
        conn,
        principal,
        &New {
            kind: "org",
            parent: value.parent,
            name: &value.name,
            name_by_person: true,
            description: &value.description,
            org: Some(hierarchy::NewOrg {
                slug: &value.slug,
                org_kind: "team",
            }),
            access: value.access,
            invitation,
        },
        value.first,
        || match before.take() {
            Some(check) => check.check(),
            None => Ok(()),
        },
    ))
    .await?;
    if value.root {
        orch_db::queries::access::add_member(
            conn,
            made.id,
            principal.user(),
            made.id,
            None,
            ctx.now,
        )
        .await?;
        orch_authz::epoch::bump(conn)
            .await
            .map_err(ApiError::internal)?;
    }
    if value.general {
        Box::pin(make_general(ctx, conn, principal, made.id)).await?;
    }
    Ok(effects::Outcome::Applied(AcceptedOrganization {
        root: value.root,
        parent: value.parent,
        id: made.id,
    }))
}
async fn finish<F: effects::Apply<Source = Source, Output = AcceptedOrganization>>(
    ctx: Ctx,
) -> ApiResult<OrgCreateResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<F>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = OrgCreateResponse::Receipt(super::super::creation_operation::receipt(
        done.output.id,
        NodeKind::Org,
    ));
    publication::receipt(&done.ctx, &receipt)?;
    Ok(
        reply::render::<Content>(&done.ctx, &done.accepted, done.output)
            .await?
            .map_or(receipt, OrgCreateResponse::full),
    )
}
pub(super) async fn root(ctx: Ctx, (): (), _body: OrgCreateBody) -> ApiResult<OrgCreateResponse> {
    finish::<Root>(ctx).await
}
pub(super) async fn child(
    ctx: Ctx,
    (): (),
    _body: OrgChildCreateBody,
) -> ApiResult<OrgCreateResponse> {
    finish::<Child>(ctx).await
}
struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = AcceptedOrganization;
    type Subject = Id;
    type Value = Org;
    type Reply = Org;
    const READ_SOURCE: Source = Source::Created;
    fn declaration(input: &Self::Input) -> &'static Declaration<Source> {
        declaration(input.root)
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let target = targets::node(conn, input.id).await?;
            let dest = match input.parent {
                Some(id) => targets::node(conn, id).await?,
                None => targets::account(conn, ctx.user()?.id()).await?,
            };
            Ok((
                input.id,
                vec![
                    native::Facts::one(
                        Source::Destination,
                        KeyTargetKind::Object(ObjectKind::Org),
                        dest,
                    ),
                    native::Facts::one(
                        Source::Created,
                        KeyTargetKind::Object(ObjectKind::Org),
                        target,
                    ),
                ],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                *subject,
                authorization::VIEW,
                orch_authz::Act::Use,
                Policy::default(),
            )
            .await
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(Org, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (mut orgs, names) = super::super::projection::project_orgs(
                ctx,
                conn,
                principal,
                principal.caller(),
                declaration(input.root),
                Source::Names,
                &[*subject],
            )
            .await?;
            Ok((orgs.pop().ok_or_else(ApiError::not_found)?, vec![names]))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Org>> {
        Box::pin(async move { Ok(value.value.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject == fresh.subject
                && old.value.base.parent_id == fresh.value.base.parent_id)
        })
    }
}
