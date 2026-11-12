//! One effect-free native readback for accepted opening creation or client/effect replay.

use super::*;

use crate::key_operations::native::reply::Prepared;

use crate::key_operations::native::reply::Projection;

use crate::routes::access::page::NameBounds;
// Thread requests and reviewed openings share this native readback and every metadata cut.
pub(in crate::routes::ports) struct Opening;

pub(in crate::routes::ports) struct Subject {
    row: rows::Opening,
    environment: EnvironmentFull,
}

pub(in crate::routes::ports) struct Value {
    names: NameBounds,
    visible: BTreeSet<Id>,
}

const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::NamesOrgs),
    (ObjectKind::Project, Source::NamesProjects),
    (ObjectKind::Topic, Source::NamesTopics),
    (ObjectKind::Thread, Source::NamesThreads),
];

impl Projection for Opening {
    type Source = Source;
    type Input = Accepted;
    type Subject = Subject;
    type Value = Value;
    type Reply = PortOpenReply;
    const READ_SOURCE: Source = Source::OpeningEnvironment;

    fn declaration(input: &Accepted) -> &'static Declaration<Source> {
        if input.request_thread.is_some() {
            &sources::THREAD_REQUEST
        } else {
            &sources::OPENING_CREATE
        }
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::opening(conn, input.row.id)
                .await?
                .filter(|row| row.environment_id == input.row.environment_id)
                .ok_or_else(ApiError::not_found)?;
            let environment = environment(conn, row.environment_id).await?;
            let facts = known(ctx, conn, input, &row).await?;
            Ok((Subject { row, environment }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        if let Some(thread) = input.request_thread {
            return Box::pin(crate::middleware::authorization::require(
                conn,
                principal,
                thread,
                crate::middleware::authorization::VIEW,
                orch_authz::Act::Use,
                Policy::default(),
            ));
        }
        Box::pin(opening_operation::require_manage(
            ctx,
            conn,
            principal,
            subject.environment.id,
        ))
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = &subject.row;
            let (hosts, publishers) = opening_operation::foreign_sources(conn, row).await?;
            let groups = |family, targets: Vec<_>| {
                if targets.is_empty() {
                    Vec::new()
                } else {
                    vec![(family, targets)]
                }
            };
            let mut additional = vec![
                native::Facts::collection(
                    Source::OpeningHosts,
                    groups(KeyTargetKind::Object(ObjectKind::Host), hosts),
                ),
                native::Facts::collection(
                    Source::OpeningPublishers,
                    groups(KeyTargetKind::Object(ObjectKind::Machine), publishers),
                ),
            ];
            let source = opening_operation::name_sources(row).nodes;
            let graph =
                orch_authz::engine::Graph::load(conn, &source.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(ApiError::internal)?;
            let ids: BTreeSet<Id> = source
                .iter()
                .flat_map(|id| {
                    graph
                        .get(*id)
                        .into_iter()
                        .flat_map(|node| node.path.iter().copied())
                })
                .chain(source.iter().copied())
                .collect();
            let roots = known(ctx, conn, input, row).await?;
            let declaration = Self::declaration(input);
            let plan = native::bind_known(ctx, declaration, &roots)?;
            let (facts, selected) = native::node_name_facts(
                &plan,
                declaration,
                principal.caller(),
                &graph,
                &ids,
                NAMES,
            )?;
            additional.extend(facts);
            let names = NameBounds {
                limited: native::modern(principal.caller()),
                nodes: selected,
                ..Default::default()
            };
            let refs = names
                .refs(conn, principal, &ids.iter().copied().collect::<Vec<_>>())
                .await?;
            let visible = refs
                .into_iter()
                .filter_map(|(id, node)| (!node.redacted && node.name.is_some()).then_some(id))
                .collect();
            Ok((Value { names, visible }, additional))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<PortOpenReply>> {
        Box::pin(async move {
            let opening = opening_view_with_names(
                ctx,
                conn,
                principal,
                &prepared.subject.row,
                &prepared.subject.environment,
                false,
                Some(&prepared.value.names),
            )
            .await?;
            let request = match (input.request_thread, prepared.subject.row.approval_id) {
                (Some(_), Some(approval)) => Some(
                    crate::routes::port_requests::request_ref(
                        conn,
                        principal,
                        approval,
                        &prepared.subject.environment,
                    )
                    .await?,
                ),
                _ => None,
            };
            Ok(PortOpenReply {
                opening,
                request,
                link: input.link.clone(),
            })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Accepted,
        opened: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let old = &opened.subject.row;
            let current = &fresh.subject.row;
            Ok(old.id == current.id
                && old.environment_id == current.environment_id
                && old.kind == current.kind
                && old.place_id == current.place_id
                && old.publish_machine_id == current.publish_machine_id
                && old.host_id == current.host_id
                && old.requested_by_thread == current.requested_by_thread
                && old.approval_id == current.approval_id
                && old.port == current.port
                && old.admission == current.admission
                && opened.value.visible.is_subset(&fresh.value.visible))
        })
    }
}

/// The native stored audience supplies the response source, never the consumed review request.
async fn named_row(conn: &mut Conn, row: &rows::Opening) -> ApiResult<native::Facts<Source>> {
    let groups = if row.kind == "place" {
        let target = targets::node(conn, row.place_id.ok_or_else(ApiError::not_found)?).await?;
        vec![(target.object.kind, vec![target])]
    } else {
        Vec::new()
    };
    Ok(native::Facts::collection(Source::NamedPlace, groups))
}

async fn known(
    ctx: &Ctx,
    conn: &mut Conn,
    input: &Accepted,
    row: &rows::Opening,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = vec![primary(conn, row.environment_id).await?];
    if let Some(thread) = input.request_thread {
        if ctx.id("id")? != thread {
            return Err(ApiError::not_found());
        }
        facts.push(native::Facts::one(
            Source::RequestThread,
            KeyTargetKind::Object(ObjectKind::Thread),
            targets::node(conn, thread).await?,
        ));
    } else {
        facts.push(named_row(conn, row).await?);
    }
    Ok(facts)
}
