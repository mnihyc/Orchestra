//! Existing native fleet fields from the actual accepted candidate snapshots, under current Read.

use super::*;
use crate::key_operations::native::reply::{Prepared, Projection};
use crate::routes::access::page::NameBounds;
use crate::routes::nodes::view::{self as node_view, ReferenceDisclosure};

pub(super) struct Reply;
pub(super) struct Subject;
pub(super) struct Value {
    native: environments::PreparedInventory,
    names: BTreeSet<Id>,
    foreign: BTreeSet<(ObjectKind, Id)>,
    manageable: BTreeSet<Id>,
    disclosure: ReferenceDisclosure,
}

impl Projection for Reply {
    type Source = Source;
    type Input = Recycled;
    type Subject = Subject;
    type Value = Value;
    type Reply = api::RecycleReply;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Recycled) -> &'static Declaration<Source> {
        &RECYCLE
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Recycled,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = input
                .snapshots
                .iter()
                .map(|snapshot| snapshot.row.id)
                .collect::<Vec<_>>();
            for snapshot in &input.snapshots {
                if !snapshot.current(conn).await? {
                    return Err(ApiError::not_found());
                }
            }
            Ok((
                Subject,
                vec![
                    facts(conn, Source::Environments, &ids).await?,
                    facts(conn, Source::Content, &ids).await?,
                ],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Recycled,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for snapshot in &input.snapshots {
                if environments::reading::decide(conn, principal, snapshot.row.id).await?
                    != environments::reading::Reading::Whole
                {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the native candidate body is no longer readable",
                    ));
                }
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Recycled,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = input
                .snapshots
                .iter()
                .map(|snapshot| snapshot.row.id)
                .collect::<Vec<_>>();
            let homes = input
                .snapshots
                .iter()
                .map(|snapshot| snapshot.row.home_node_id)
                .collect::<Vec<_>>();
            let mut native = environments::prepare_shown_rows(
                conn,
                principal,
                input
                    .snapshots
                    .iter()
                    .map(|snapshot| snapshot.row.clone())
                    .collect(),
            )
            .await?;
            let known = native::bind_known(
                ctx,
                &RECYCLE,
                &[
                    facts(conn, Source::Environments, &ids).await?,
                    facts(conn, Source::Content, &ids).await?,
                ],
            )?;
            let disclosure = node_view::reference_disclosure(
                conn,
                principal,
                &homes,
                orch_authz::Policy::default(),
            )
            .await?;
            let mut wanted = orch_authz::resources::address_name_sources(conn, &ids)
                .await
                .map_err(ApiError::internal)?;
            wanted.extend(disclosure.names.iter().copied());
            let wanted = wanted.into_iter().collect::<BTreeSet<_>>();
            let graph =
                orch_authz::engine::Graph::load(conn, &wanted.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(ApiError::internal)?;
            let (mut extra, names) = native::node_name_facts(
                &known,
                &RECYCLE,
                principal.caller(),
                &graph,
                &wanted,
                &[
                    (ObjectKind::Org, Source::Orgs),
                    (ObjectKind::Project, Source::Projects),
                    (ObjectKind::Topic, Source::Topics),
                    (ObjectKind::Thread, Source::Threads),
                ],
            )?;
            native.bound_names(NameBounds {
                limited: native::modern(principal.caller()),
                nodes: names.clone(),
                ..Default::default()
            });
            let mut groups = BTreeMap::new();
            let mut foreign = BTreeSet::new();
            for object in native
                .content_sources(conn)
                .await?
                .into_values()
                .flatten()
                .filter(|object| matches!(object.kind, ObjectKind::Host | ObjectKind::Template))
            {
                if !foreign.insert((object.kind, object.id)) {
                    continue;
                }
                let family = KeyTargetKind::Object(object.kind);
                let target = targets::resource_identity(conn, object).await?;
                if native::permits_usage(
                    &known,
                    &RECYCLE,
                    principal.caller(),
                    &Source::Foreign,
                    FacetUsage::ResponseContent,
                    family,
                    &target,
                )? {
                    groups.entry(family).or_insert_with(Vec::new).push(target);
                } else {
                    foreign.remove(&(object.kind, object.id));
                }
            }
            extra.push(native::Facts::collection(
                Source::Foreign,
                groups.into_iter().collect(),
            ));
            let mut manageable = BTreeSet::new();
            for id in &ids {
                if updates::may_manage(conn, principal, Target::Environment(*id))
                    .await
                    .map_err(ApiError::internal)?
                {
                    manageable.insert(*id);
                }
            }
            Ok((
                Value {
                    native,
                    names,
                    foreign,
                    manageable,
                    disclosure,
                },
                extra,
            ))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Recycled,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::RecycleReply>> {
        Box::pin(async move {
            let views =
                environments::show_inventory_prepared(ctx, conn, principal, &prepared.value.native)
                    .await?;
            let mut candidates = Vec::new();
            for view in views {
                let snapshot = input
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.row.id == view.base.id)
                    .ok_or_else(ApiError::not_found)?;
                let host = if !native::modern(principal.caller())
                    || snapshot.host.is_some_and(|host| {
                        prepared.value.foreign.contains(&(ObjectKind::Host, host))
                    }) {
                    view.host.map(|host| host.name)
                } else {
                    None
                };
                let template = view
                    .template
                    .filter(|template| {
                        !native::modern(principal.caller())
                            || prepared
                                .value
                                .foreign
                                .contains(&(ObjectKind::Template, template.id))
                    })
                    .map(|template| api::TemplateRef {
                        id: template.id,
                        name: template.name,
                    });
                candidates.push(api::FleetEntry {
                    environment: view.base,
                    home: Some(view.home),
                    threads: snapshot.threads,
                    host_name: host,
                    template,
                    generation: view.generation,
                    updates: i64::try_from(view.updates.len()).unwrap_or(i64::MAX),
                    last_activity_at: view.last_activity_at,
                    idle_minutes: view
                        .last_activity_at
                        .map(|at| (snapshot.at.get() - at.get()).num_minutes().max(0)),
                    idle_stop_at: view.idle_stop_at,
                    active_turns: snapshot.active_turns,
                    open_shells: snapshot.open_shells,
                    processes: None,
                    cpu_percent: None,
                    memory_bytes: None,
                    memory_limit_bytes: None,
                    disk_used_bytes: None,
                    manageable: prepared.value.manageable.contains(&snapshot.row.id),
                    holds: Vec::new(),
                });
            }
            Ok(api::RecycleReply {
                candidates,
                stopped: input.stopped,
                skipped_no_permission: input.skipped_no_permission,
                skipped_active: input.skipped_active,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Recycled,
        opened: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.value.native.preserves_sources(&fresh.value.native)
                && opened.value.names.is_subset(&fresh.value.names)
                && opened.value.foreign.is_subset(&fresh.value.foreign)
                && opened.value.manageable.is_subset(&fresh.value.manageable)
                && opened
                    .value
                    .disclosure
                    .names
                    .is_subset(&fresh.value.disclosure.names)
                && opened
                    .value
                    .disclosure
                    .displays
                    .is_subset(&fresh.value.disclosure.displays))
        })
    }
}
