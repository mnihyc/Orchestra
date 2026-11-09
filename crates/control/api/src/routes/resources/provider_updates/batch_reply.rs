//! Native accepted batch identities use the shared current completed-read projector.
use super::*;
use crate::key_operations::{
    native::{self, reply},
    publication, targets,
};
use crate::middleware::idempotency::Creation;
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    common::{ErrorCode, ReceiptProjection},
    operation_sources::Declaration,
    operations::FacetUsage,
    provider_update_sources::{CREATE, Source},
};
use orch_db::{
    idempotency::creation::Created,
    queries::jobs::{self, JobRow},
};
use std::collections::{BTreeSet, HashMap};

const KIND: &str = "provider_runtime_update_batch";
#[derive(serde::Serialize, serde::Deserialize)]
struct Accepted {
    jobs: Vec<(Id, Id)>,
}
struct Input {
    batch: Id,
    accepted: Accepted,
    first: Option<Vec<JobRow>>,
}
struct Value {
    names: NameBounds,
    machines: HashMap<Id, String>,
}
struct Content;

pub(super) fn binding() -> Creation<
    orch_api_contract::forks::UpdateConfirmQuery,
    ForkCreateBody,
    api::ProviderBatchResponse,
> {
    Creation {
        kind: KIND,
        identity: |value| {
            Ok(Created {
                kind: KIND.into(),
                id: match value {
                    api::ProviderBatchResponse::Full(value) => value.batch_id,
                    api::ProviderBatchResponse::Receipt(value) => value.batch_id,
                },
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, _, _, created| {
            Box::pin(async move {
                if created.kind != KIND {
                    return Err(ApiError::internal("the accepted native batch kind changed"));
                }
                let accepted: Accepted =
                    serde_json::from_value(created.data).map_err(ApiError::internal)?;
                respond(
                    &ctx,
                    Input {
                        batch: created.id,
                        accepted,
                        first: None,
                    },
                )
                .await
            })
        },
    }
}
pub(super) async fn observed(
    conn: &mut Conn,
    account: Id,
    batch: Id,
    rows: &[JobRow],
) -> ApiResult<()> {
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(account),
        KIND,
        batch,
        serde_json::to_value(accepted(rows)?).map_err(ApiError::internal)?,
    )
    .await?;
    Ok(())
}
fn accepted(rows: &[JobRow]) -> ApiResult<Accepted> {
    Ok(Accepted {
        jobs: rows
            .iter()
            .map(|row| Ok((row.provider_id.ok_or_else(ApiError::not_found)?, row.id)))
            .collect::<ApiResult<_>>()?,
    })
}
pub(super) async fn first(
    ctx: &Ctx,
    batch: Id,
    rows: Vec<JobRow>,
) -> ApiResult<api::ProviderBatchResponse> {
    let accepted = accepted(&rows)?;
    respond(
        ctx,
        Input {
            batch,
            accepted,
            first: Some(rows),
        },
    )
    .await
}
async fn respond(ctx: &Ctx, input: Input) -> ApiResult<api::ProviderBatchResponse> {
    let receipt = api::ProviderBatchReceipt {
        projection: ReceiptProjection::Receipt,
        batch_id: input.batch,
        jobs: input
            .accepted
            .jobs
            .iter()
            .map(|(provider_id, job_id)| api::ProviderBatchJobReceipt {
                provider_id: *provider_id,
                job_id: *job_id,
            })
            .collect(),
    };
    publication::receipt(ctx, &receipt)?;
    match reply::render_completed_replay::<Content>(ctx, ctx.user()?.id(), input).await {
        Ok(Some(value)) => Ok(api::ProviderBatchResponse::full(value)),
        Ok(None) => Ok(api::ProviderBatchResponse::Receipt(receipt)),
        Err(e) if matches!(e.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
            Ok(api::ProviderBatchResponse::Receipt(receipt))
        }
        Err(e) => Err(e),
    }
}
impl reply::Projection for Content {
    type Source = Source;
    type Input = Input;
    type Subject = Vec<JobRow>;
    type Value = Value;
    type Reply = api::ProviderBatch;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(Vec<JobRow>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut rows = Vec::new();
            let mut providers = Vec::new();
            for (provider, job) in &input.accepted.jobs {
                let row = jobs::job(conn, *job)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if row.kind != "provider_recreate"
                    || row.provider_id != Some(*provider)
                    || row.environment_id.is_some()
                    || row.batch_id != Some(input.batch)
                    || row.created_by != Some(ctx.user()?.id())
                {
                    return Err(ApiError::not_found());
                }
                providers.push(
                    targets::resource_identity(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Provider,
                            id: *provider,
                        },
                    )
                    .await?,
                );
                rows.push(row);
            }
            let groups = if providers.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Provider), providers)]
            };
            Ok((
                rows,
                vec![native::Facts::collection(Source::Content, groups)],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        _: &'a Input,
        rows: &'a Vec<JobRow>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for row in rows {
                if !super::reference_access(
                    conn,
                    p,
                    row.provider_id.ok_or_else(ApiError::not_found)?,
                )
                .await?
                {
                    return Err(ApiError::not_found());
                }
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
        rows: &'a Vec<JobRow>,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = rows
                .iter()
                .filter_map(|row| row.provider_id)
                .collect::<Vec<_>>();
            let mut providers = Vec::new();
            for id in &ids {
                providers.push(
                    targets::resource_identity(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Provider,
                            id: *id,
                        },
                    )
                    .await?,
                );
            }
            let groups = if providers.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Provider), providers)]
            };
            let known = native::bind_known(
                ctx,
                &CREATE,
                &[native::Facts::collection(Source::Content, groups)],
            )?;
            let wanted = orch_authz::resources::address_name_sources(conn, &ids)
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .collect::<BTreeSet<_>>();
            let graph =
                orch_authz::engine::Graph::load(conn, &wanted.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(ApiError::internal)?;
            let (mut facts, names) = native::node_name_facts(
                &known,
                &CREATE,
                p.caller(),
                &graph,
                &wanted,
                &[
                    (ObjectKind::Org, Source::Orgs),
                    (ObjectKind::Project, Source::Projects),
                    (ObjectKind::Topic, Source::Topics),
                    (ObjectKind::Thread, Source::Threads),
                ],
            )?;
            let mut machines = if input.first.is_some() {
                HashMap::new()
            } else {
                shown_jobs::machine_name_sources(conn, rows).await?
            };
            let mut machine_targets = Vec::new();
            for id in machines.keys().copied().collect::<Vec<_>>() {
                let target = targets::machine_subject(conn, id).await?;
                if native::permits_usage(
                    &known,
                    &CREATE,
                    p.caller(),
                    &Source::Machines,
                    FacetUsage::ResponseContent,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    &target,
                )? {
                    machine_targets.push(target);
                } else {
                    machines.remove(&id);
                }
            }
            facts.push(native::Facts::collection(Source::Hosts, Vec::new()));
            facts.push(native::Facts::collection(Source::Templates, Vec::new()));
            facts.push(native::Facts::collection(
                Source::Machines,
                if machine_targets.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Machine), machine_targets)]
                },
            ));
            Ok((
                Value {
                    names: NameBounds {
                        limited: native::modern(p.caller()),
                        nodes: names,
                        ..Default::default()
                    },
                    machines,
                },
                facts,
            ))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::ProviderBatch>> {
        Box::pin(async move {
            let ids = opened
                .subject
                .iter()
                .filter_map(|row| row.provider_id)
                .collect::<Vec<_>>();
            let providers = super::super::view::provider_refs_with_names(
                conn,
                &ids,
                &[],
                Some(&opened.value.names),
            )
            .await?
            .into_iter()
            .map(|(_, row)| (row.id, row))
            .collect::<HashMap<_, _>>();
            let rows = input.first.as_deref().unwrap_or(&opened.subject);
            let mut jobs = Vec::new();
            for row in rows {
                jobs.push(api::ProviderBatchJob {
                    provider: providers
                        .get(&row.provider_id.ok_or_else(ApiError::not_found)?)
                        .ok_or_else(ApiError::not_found)?
                        .clone(),
                    job: shown_jobs::view_with_machine_sources(
                        conn,
                        row,
                        Some(&opened.value.machines),
                    )
                    .await?,
                });
            }
            Ok(api::ProviderBatch {
                batch_id: input.batch,
                jobs,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a reply::Prepared<Self>,
        new: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.len() == new.subject.len()
                && old.subject.iter().zip(&new.subject).all(|(a, b)| {
                    a.id == b.id
                        && a.provider_id == b.provider_id
                        && a.batch_id == b.batch_id
                        && a.created_by == b.created_by
                        && a.kind == b.kind
                })
                && old.value.names.nodes.is_subset(&new.value.names.nodes)
                && old
                    .value
                    .machines
                    .keys()
                    .all(|id| new.value.machines.contains_key(id)))
        })
    }
}
