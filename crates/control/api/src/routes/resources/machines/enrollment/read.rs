//! Issuer recovery retains its original pending or completed native source through publication.

use super::RECEIPT_BYTES;
use crate::key_operations::{native, publication};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx};
use crate::routes::access::operations::projection_names;
use crate::routes::access::page::NameBounds;
use crate::routes::resources::{
    hosts,
    machines::{individual, read as machine_read},
};
use futures::future::BoxFuture;
use http::header::CACHE_CONTROL;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::machine_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::resources::EnrollmentProgress;
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_types::Id;
use std::collections::BTreeSet;
use std::sync::Arc;

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

pub(super) struct Progress;
#[derive(PartialEq, Eq)]
struct Saved {
    receipt: String,
    purpose: String,
    machine: Option<Id>,
    resource: Option<ObjectRef>,
}
pub(super) struct Opened {
    saved: Saved,
    view: View,
}
enum View {
    Pending,
    Machine(Arc<individual::Opened>),
    Host(Box<Host>),
}
struct Host {
    native: hosts::PreparedInventory,
    bounds: NameBounds,
    named_nodes: BTreeSet<Id>,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the original enrollment recovery source is no longer admitted",
    )
}

async fn saved(ctx: &Ctx, conn: &mut Conn, earlier: Option<&Saved>) -> ApiResult<Saved> {
    let receipt = ctx.param("receipt")?;
    let fingerprint = hex::decode(receipt)
        .ok()
        .filter(|value| value.len() == RECEIPT_BYTES)
        .ok_or_else(|| ApiError::invalid("receipt", "must name an enrollment"))?;
    let (purpose, machine) =
        orch_db::queries::resources::enrollment_progress(conn, ctx.user()?.id(), &fingerprint)
            .await?
            .ok_or_else(ApiError::not_found)?;
    if let Some(earlier) = earlier {
        if earlier.receipt != receipt || earlier.purpose != purpose {
            return Err(refused());
        }
        // A frozen pending body contains no completed-resource metadata. Later completion
        // does not turn its empty selected-source footprint into a different response.
        if earlier.resource.is_none() {
            return Ok(Saved {
                receipt: receipt.to_owned(),
                purpose,
                machine: None,
                resource: None,
            });
        }
        if machine != earlier.machine {
            return Err(refused());
        }
    }
    let resource = match machine {
        None => None,
        Some(machine) if purpose == "host" => {
            let host = sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id=$1")
                .bind(machine)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?
                .ok_or_else(ApiError::not_found)?;
            Some(ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            })
        }
        Some(machine) => Some(ObjectRef {
            kind: ObjectKind::Machine,
            id: machine,
        }),
    };
    if earlier.is_some_and(|earlier| earlier.resource != resource) {
        return Err(refused());
    }
    Ok(Saved {
        receipt: receipt.to_owned(),
        purpose,
        machine,
        resource,
    })
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    earlier: Option<&Saved>,
) -> ApiResult<(Opened, Vec<native::Facts<Source>>)> {
    if ctx.user()?.id() != caller.user() {
        return Err(ApiError::internal("enrollment recovery changed its issuer"));
    }
    let mut facts = vec![native::Facts::one(
        Source::Issuer,
        ACCOUNT,
        crate::key_operations::targets::account(conn, caller.user()).await?,
    )];
    let known = native::bind_known(ctx, &sources::ENROLLMENT, &facts)?;
    let _issuer = native::preparation_principal(ctx, conn, caller, &known).await?;
    let saved = saved(ctx, conn, earlier).await?;
    let view = match saved.resource {
        None => {
            facts.extend(
                sources::ENROLLMENT
                    .bindings
                    .iter()
                    .filter(|binding| binding.source != Source::Issuer)
                    .map(|binding| native::Facts::collection(binding.source, Vec::new())),
            );
            View::Pending
        }
        Some(ObjectRef {
            kind: ObjectKind::Machine,
            id,
        }) => {
            let (value, completed) =
                individual::prepare_for(ctx, conn, caller, id, &sources::ENROLLMENT, facts).await?;
            facts = completed;
            View::Machine(value)
        }
        Some(
            object @ ObjectRef {
                kind: ObjectKind::Host,
                ..
            },
        ) => {
            let roots = crate::routes::nodes::read_sources::Sources::from_objects([object])
                .identity_targets(conn)
                .await?;
            let target = roots
                .into_iter()
                .find_map(|(source, target)| (source == object).then_some(target))
                .ok_or_else(ApiError::not_found)?;
            facts.push(native::Facts::one(
                Source::Governor,
                KeyTargetKind::Object(ObjectKind::Host),
                target,
            ));
            let known = native::bind_known(ctx, &sources::ENROLLMENT, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            crate::routes::resources::require(
                conn,
                &principal,
                ResourceKind::Host,
                object.id,
                Permission::HostView,
            )
            .await?;
            let mut prepared = hosts::prepare_single(ctx, conn, &principal, object.id).await?;
            let candidates: Vec<_> = prepared
                .retained_name_sources()
                .iter()
                .map(|id| ObjectRef {
                    kind: ObjectKind::Environment,
                    id: *id,
                })
                .collect();
            let candidates = machine_read::registered_facts(conn, &candidates).await?;
            let mut allowed = BTreeSet::new();
            let mut selected = Vec::new();
            for (kind, targets) in candidates.groups {
                if kind != ENVIRONMENT {
                    return Err(ApiError::internal(
                        "Host recovery has an undeclared retained source",
                    ));
                }
                for target in targets {
                    if !native::modern(caller)
                        || native::permits(
                            &known,
                            &sources::ENROLLMENT,
                            caller,
                            &Source::Registered,
                            ENVIRONMENT,
                            &target,
                        )?
                    {
                        allowed.insert(target.object.id.ok_or_else(ApiError::not_found)?);
                        selected.push(target);
                    }
                }
            }
            prepared.bound_retained_sources(conn, allowed).await?;
            facts.push(native::Facts::collection(
                Source::Registered,
                if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![(ENVIRONMENT, selected)]
                },
            ));
            facts.push(native::Facts::collection(
                Source::ReadinessMachines,
                Vec::new(),
            ));
            let mut plan = native::bind_known(ctx, &sources::ENROLLMENT, &facts)?;
            let bounds = projection_names(
                conn,
                caller,
                &mut plan,
                prepared.name_sources().clone(),
                &machine_read::NAMES,
            )
            .await?;
            let named_nodes = machine_read::named_nodes(conn, &principal, &bounds.nodes).await?;
            facts.extend(machine_read::projection_facts(&plan)?);
            prepared.bound_names(bounds.clone());
            View::Host(Box::new(Host {
                native: prepared,
                bounds,
                named_nodes,
            }))
        }
        Some(_) => {
            return Err(ApiError::internal(
                "enrollment recovery selected the wrong resource family",
            ));
        }
    };
    Ok((Opened { saved, view }, facts))
}

impl Opened {
    fn preserved_by(&self, fresh: &Self) -> bool {
        self.saved == fresh.saved
            && match (&self.view, &fresh.view) {
                (View::Pending, View::Pending) => true,
                (View::Machine(old), View::Machine(current)) => old.preserved_by(current),
                (View::Host(old), View::Host(current)) => {
                    old.native.preserves_sources(&current.native)
                        && old.bounds.nodes.is_subset(&current.bounds.nodes)
                        && old.named_nodes.is_subset(&current.named_nodes)
                }
                _ => false,
            }
    }
}

impl native::Selected for Progress {
    type Source = Source;
    type Query = ();
    type Value = Opened;
    type Reply = EnrollmentProgress;
    const DECLARATION: &'static Declaration<Source> = &sources::ENROLLMENT;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Opened, Vec<native::Facts<Source>>)>> {
        Box::pin(select(ctx, conn, caller, None))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Opened,
    ) -> BoxFuture<'a, ApiResult<EnrollmentProgress>> {
        Box::pin(async move {
            let mut result = EnrollmentProgress {
                receipt: value.saved.receipt.clone(),
                machine: None,
                host: None,
            };
            match &value.view {
                View::Pending => {}
                View::Machine(prepared) => {
                    result.machine = Some(Box::new(prepared.render(ctx, conn, principal).await?))
                }
                View::Host(prepared) => {
                    result.host = Some(Box::new(
                        hosts::show_inventory_prepared(ctx, conn, principal, &prepared.native)
                            .await?
                            .pop()
                            .ok_or_else(|| {
                                ApiError::internal("the recovered Host was not projected")
                            })?,
                    ))
                }
            }
            Ok(result)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        old: &'a Opened,
        fresh: &'a Opened,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.preserved_by(fresh)) })
    }
}

async fn check(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    opened: &native::Prepared<Opened>,
) -> ApiResult<()> {
    let (fresh, facts) = match select(ctx, conn, reader.caller(), Some(&opened.value.saved)).await {
        Err(error)
            if matches!(
                error.problem.code,
                ErrorCode::NotFound | ErrorCode::AuthzDenied
            ) =>
        {
            return Err(refused());
        }
        result => result?,
    };
    let plan = native::bind(ctx, &sources::ENROLLMENT, &facts)?;
    if !opened.value.preserved_by(&fresh) || !plan.shared().covers_sources(opened.response.shared())
    {
        return Err(refused());
    }
    if native::modern(reader.caller()) && !plan.admit(reader.caller())?.content {
        return Err(refused());
    }
    ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Full);
    if ctx.key_receipt_required() {
        return Err(refused());
    }
    Ok(())
}

/// The shared watched-read and response boundary, retaining the original progress projection.
pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<EnrollmentProgress>> {
    use native::Selected;
    let (reply, producing, accepted, opened) = native::read(ctx, |ctx, conn, principal| {
        Box::pin(async move {
            let opened = if native::modern(principal.caller()) {
                ctx.operation_data::<native::Prepared<Opened>>()?
            } else {
                let (value, facts) = Progress::select(ctx, conn, principal.caller(), &()).await?;
                Arc::new(native::Prepared {
                    response: native::bind(ctx, &sources::ENROLLMENT, &facts)?,
                    value,
                })
            };
            let reply = if native::modern(principal.caller()) {
                let producing = ctx.clone();
                let value = Arc::clone(&opened);
                opened
                    .response
                    .clone()
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            Progress::render(&producing, conn, reader, &value.value).await
                        })
                    })
                    .await?
                    .ok_or_else(refused)?
            } else {
                Progress::render(ctx, conn, principal, &opened.value).await?
            };
            Ok((reply, ctx.clone(), principal.caller().clone(), opened))
        })
    })
    .await?;
    publication::retain(
        &producing,
        &accepted,
        opened,
        |ctx, conn, reader, opened| Box::pin(check(ctx, conn, reader, opened)),
    )?;
    Ok(Answer::from(reply).header(CACHE_CONTROL, crate::sign_in::NOT_KEPT))
}
