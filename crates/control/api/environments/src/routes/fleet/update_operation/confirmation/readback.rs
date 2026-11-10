//! Current native batch/job projection after acceptance; management is not re-admitted.

use super::*;

use crate::key_operations::native::reply::Prepared;

use crate::key_operations::native::reply::Projection;

use crate::routes::nodes::view::ReferenceDisclosure;

use std::collections::BTreeSet;

use std::collections::HashMap;
pub(super) struct Reply;

pub(super) struct Subject {
    rows: Vec<orch_db::queries::jobs::JobRow>,
}

pub(super) struct Value {
    native: environments::PreparedInventory,
    names: crate::routes::access::page::NameBounds,
    disclosure: ReferenceDisclosure,
    machines: HashMap<Id, String>,
}

impl Projection for Reply {
    type Source = Source;
    type Input = Readback;
    type Subject = Subject;
    type Value = Value;
    type Reply = api::EnvironmentBatch;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Readback) -> &'static Declaration<Source> {
        row()
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if input.node() != ctx.id("id")? {
                return Err(ApiError::not_found());
            }
            let mut rows = Vec::new();
            for (environment, job) in input.jobs() {
                let row = orch_db::queries::jobs::job(conn, *job)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if row.kind != "environment_recreate"
                    || row.environment_id != Some(*environment)
                    || row.provider_id.is_some()
                    || row.batch_id != Some(input.batch())
                    || (matches!(input, Readback::Replay { .. })
                        && row.created_by != Some(ctx.user()?.id()))
                {
                    return Err(ApiError::not_found());
                }
                rows.push(row);
            }
            let ids = input.ids();
            let mut facts = input.required(conn).await?;
            facts.push(environment_facts(conn, Source::Content, &ids).await?);
            Ok((Subject { rows }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for id in input.ids() {
                environments::reading::decide(conn, principal, id).await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = input.ids();
            let rows = orch_db::queries::environments::routes::environments(conn, &ids).await?;
            if rows.len() != ids.len() {
                return Err(ApiError::not_found());
            }
            let mut native = environments::prepare_shown_rows(conn, principal, rows).await?;
            let mut facts = input.required(conn).await?;
            facts.push(environment_facts(conn, Source::Content, &ids).await?);
            let known = native::bind_known(ctx, row(), &facts)?;
            let wanted = orch_authz::resources::address_name_sources(conn, &ids)
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .collect();
            let mut names =
                super::super::names::prepare(conn, principal, &known, row(), wanted).await?;
            let wanted = shown_jobs::machine_name_sources(conn, &subject.rows).await?;
            let mut machines = HashMap::new();
            for (id, name) in wanted {
                let target = targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Machine,
                        id,
                    },
                )
                .await?;
                let family = KeyTargetKind::Object(ObjectKind::Machine);
                if native::permits_usage(
                    &known,
                    row(),
                    principal.caller(),
                    &Source::Context,
                    FacetUsage::ResponseContent,
                    family,
                    &target,
                )? {
                    machines.insert(id, name);
                    names
                        .groups
                        .entry(family)
                        .or_insert_with(Vec::new)
                        .push(target);
                }
            }
            native.bound_names(names.bounds.clone());
            Ok((
                Value {
                    native,
                    names: names.bounds,
                    disclosure: names.disclosure,
                    machines,
                },
                vec![native::Facts::collection(
                    Source::Context,
                    names.groups.into_iter().collect(),
                )],
            ))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::EnvironmentBatch>> {
        Box::pin(async move {
            let environments =
                environments::show_inventory_prepared(ctx, conn, principal, &prepared.value.native)
                    .await?
                    .into_iter()
                    .map(|environment| (environment.base.id, environment.base))
                    .collect::<BTreeMap<_, _>>();
            let mut jobs = Vec::new();
            for row in &prepared.subject.rows {
                let environment = row
                    .environment_id
                    .and_then(|id| environments.get(&id))
                    .ok_or_else(ApiError::not_found)?
                    .clone();
                jobs.push(api::EnvironmentBatchJob {
                    environment,
                    job: shown_jobs::view_with_machine_sources(
                        conn,
                        row,
                        Some(&prepared.value.machines),
                    )
                    .await?,
                });
            }
            Ok(api::EnvironmentBatch {
                batch_id: input.batch(),
                jobs,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        opened: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let old = opened
                .subject
                .rows
                .iter()
                .map(|row| (row.id, row.environment_id, row.batch_id))
                .collect::<BTreeSet<_>>();
            let new = fresh
                .subject
                .rows
                .iter()
                .map(|row| (row.id, row.environment_id, row.batch_id))
                .collect::<BTreeSet<_>>();
            Ok(old == new
                && opened.value.native.preserves_sources(&fresh.value.native)
                && opened.value.names.nodes.is_subset(&fresh.value.names.nodes)
                && opened
                    .value
                    .disclosure
                    .names
                    .is_subset(&fresh.value.disclosure.names)
                && opened
                    .value
                    .disclosure
                    .displays
                    .is_subset(&fresh.value.disclosure.displays)
                && opened
                    .value
                    .machines
                    .keys()
                    .all(|id| fresh.value.machines.contains_key(id)))
        })
    }
}
