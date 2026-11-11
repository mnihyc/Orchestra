//! Accepted share identities reopen only their native current response sources.

use super::*;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::middleware::idempotency::ObservedCreation;

use orch_api_contract::operation_sources::Binding;

use orch_api_contract::operation_sources::Cardinality;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver as Target;
type Role = &'static str;

const SOURCE: Role = "response_source";

const RECIPIENTS: [(Role, ObjectKind); 4] = [
    ("response_recipient_orgs", ObjectKind::Org),
    ("response_recipient_projects", ObjectKind::Project),
    ("response_recipient_topics", ObjectKind::Topic),
    ("response_recipient_threads", ObjectKind::Thread),
];

const NAMES: [Role; 4] = [
    "share_name_orgs",
    "share_name_projects",
    "share_name_topics",
    "share_name_threads",
];

const fn content(role: Role) -> Binding<Role> {
    Binding {
        source: role,
        target: Target::ResolvedCollection(role),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    }
}

const fn name(role: Role) -> Binding<Role> {
    Binding {
        source: role,
        target: Target::ResolvedCollection(role),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    }
}

const ROOT: Binding<Role> = Binding {
    source: SOURCE,
    target: Target::Resolved(SOURCE),
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};

const LIVE: &[Binding<Role>] = &[
    ROOT,
    content(RECIPIENTS[0].0),
    content(RECIPIENTS[1].0),
    content(RECIPIENTS[2].0),
    content(RECIPIENTS[3].0),
    name(NAMES[0]),
    name(NAMES[1]),
    name(NAMES[2]),
    name(NAMES[3]),
];

const SNAPSHOT: &[Binding<Role>] = &[
    ROOT,
    content(RECIPIENTS[3].0),
    name(NAMES[0]),
    name(NAMES[1]),
    name(NAMES[2]),
    name(NAMES[3]),
];

static ROWS: [Declaration<Role>; 6] = [
    Declaration {
        operation: "ShareAgentCreate.live",
        bindings: LIVE,
    },
    Declaration {
        operation: "ShareAgentCreate.live_viewers_read",
        bindings: LIVE,
    },
    Declaration {
        operation: "ShareAgentCreate.live_read_first",
        bindings: LIVE,
    },
    Declaration {
        operation: "ShareAgentCreate.live_read_first_viewers_read",
        bindings: LIVE,
    },
    Declaration {
        operation: "ShareAgentCreate.snapshot",
        bindings: SNAPSHOT,
    },
    Declaration {
        operation: "ShareAgentCreate.snapshot_comment",
        bindings: SNAPSHOT,
    },
];

fn row(selection: &str) -> ApiResult<&'static Declaration<Role>> {
    ROWS.iter()
        .find(|row| row.operation == selection)
        .ok_or_else(|| ApiError::internal("the accepted share has an unknown creating operation"))
}

struct Accepted {
    declaration: &'static Declaration<Role>,
    expected: Subject,
    receipt: ShareAgentCreateReceipt,
}

struct View {
    records: Vec<Record>,
    projection: ShareProjection,
}

struct Content;

async fn load(ctx: &Ctx, conn: &mut Conn, input: &Accepted) -> ApiResult<Vec<Record>> {
    let mut records = Vec::with_capacity(input.receipt.shares.len());
    for accepted in &input.receipt.shares {
        let mut record = ctx
            .app
            .engine
            .agent_share_record_in(conn, accepted.id, ctx.app.clock.now())
            .await
            .map_err(|error| match error {
                ShareError::NotFound => ApiError::not_found(),
                ShareError::Internal(error) => ApiError::engine(error),
                _ => ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the accepted share is no longer readable",
                ),
            })?;
        if !orch_engine::sharing::creation::matches_subject(conn, &mut record, input.expected)
            .await?
            || record.to != accepted.to.id
            || record.mode != accepted.mode
        {
            return Err(ApiError::not_found());
        }
        record.fresh = accepted.created;
        records.push(record);
    }
    Ok(records)
}

async fn facts(
    conn: &mut Conn,
    input: &Accepted,
    records: &[Record],
) -> ApiResult<Vec<native::Facts<Role>>> {
    let target = source(conn, &what_of(input.expected)).await?;
    let mut facts = vec![native::Facts::one(SOURCE, target.object.kind, target)];
    for (role, kind) in RECIPIENTS.iter().filter(|(role, _)| {
        input
            .declaration
            .bindings
            .iter()
            .any(|binding| binding.source == *role)
    }) {
        let mut selected = Vec::new();
        for record in records {
            let target = targets::node(conn, record.to).await?;
            if target.object.kind == KeyTargetKind::Object(*kind)
                && !selected
                    .iter()
                    .any(|old: &KeyResolvedTarget| old.object == target.object)
            {
                selected.push(target);
            }
        }
        facts.push(native::Facts::collection(
            *role,
            if selected.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(*kind), selected)]
            },
        ));
    }
    Ok(facts)
}

async fn discovery(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    input: &Accepted,
    records: &[Record],
) -> ApiResult<Principal> {
    let facts = facts(conn, input, records).await?;
    let known = native::bind_known(ctx, input.declaration, &facts)?;
    native::preparation_principal(ctx, conn, principal.caller(), &known).await
}

impl reply::Projection for Content {
    type Source = Role;
    type Input = Accepted;
    type Subject = Vec<Record>;
    type Value = View;
    type Reply = ShareAgentCreateReply;
    const READ_SOURCE: Role = SOURCE;
    fn declaration(input: &Accepted) -> &'static Declaration<Role> {
        input.declaration
    }
    fn replay_read_sources(_: &Accepted) -> &'static [Role] {
        &NAMES
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> futures::future::BoxFuture<'a, ApiResult<(Vec<Record>, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let records = load(ctx, conn, input).await?;
            let facts = facts(conn, input, &records).await?;
            Ok((records, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        records: &'a Vec<Record>,
    ) -> futures::future::BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let reader = discovery(ctx, conn, principal, input, records).await?;
            for record in records {
                let checked = ctx
                    .app
                    .engine
                    .check_agent_share_source_in(conn, &reader, &origin(ctx), record.subject)
                    .await;
                if let Err(error) = checked {
                    return Err(failure(ctx, conn, error).await);
                }
                authorization::require(
                    conn,
                    &reader,
                    record.to,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        records: &'a Vec<Record>,
    ) -> futures::future::BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let reader = discovery(ctx, conn, principal, input, records).await?;
            let facts = facts(conn, input, records).await?;
            let known = native::bind_known(ctx, input.declaration, &facts)?;
            let wanted = super::records::wanted_names(conn, &reader, records).await?;
            let allowed = name_bindings(ctx, conn, principal.caller(), &known, &wanted).await?;
            let bounds = crate::routes::access::page::NameBounds {
                limited: native::modern(principal.caller()),
                nodes: allowed
                    .iter()
                    .filter_map(|target| target.object.id)
                    .collect(),
                containers: Default::default(),
            };
            let projection =
                project_shares(ctx, conn, &reader, records, None, Some(&bounds)).await?;
            if projection.shares.len() != records.len() {
                return Err(ApiError::not_found());
            }
            let mut names = Vec::new();
            for ((kind, _), role) in NODES.iter().zip(NAMES) {
                let actual: Vec<_> = allowed
                    .iter()
                    .filter(|target| {
                        target.object.kind == KeyTargetKind::Object(*kind)
                            && target
                                .object
                                .id
                                .is_some_and(|id| projection.named.contains(&id))
                    })
                    .cloned()
                    .collect();
                names.push(native::Facts::collection(
                    role,
                    if actual.is_empty() {
                        Vec::new()
                    } else {
                        vec![(KeyTargetKind::Object(*kind), actual)]
                    },
                ));
            }
            Ok((
                View {
                    records: records.clone(),
                    projection,
                },
                names,
            ))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        prepared: &'a reply::Prepared<Self>,
    ) -> futures::future::BoxFuture<'a, ApiResult<ShareAgentCreateReply>> {
        Box::pin(async move {
            Ok(ShareAgentCreateReply {
                shares: prepared.value.projection.shares.clone(),
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
    ) -> futures::future::BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old
                .value
                .records
                .iter()
                .zip(&current.value.records)
                .all(|(old, new)| {
                    old.id == new.id
                        && old.subject == new.subject
                        && old.to == new.to
                        && old.by == new.by
                        && old.mode == new.mode
                        && old.grants.iter().all(|entry| {
                            new.grants
                                .iter()
                                .any(|current| current.entry_id == entry.entry_id)
                        })
                        && old.copy.as_ref().map(|copy| (copy.id, &copy.path))
                            == new.copy.as_ref().map(|copy| (copy.id, &copy.path))
                })
                && old.value.records.len() == current.value.records.len()
                && old
                    .value
                    .projection
                    .named
                    .is_subset(&current.value.projection.named)
                && old
                    .value
                    .projection
                    .inputs
                    .iter()
                    .all(|input| current.value.projection.inputs.contains(input)))
        })
    }
}

pub(in crate::routes::share) async fn render(
    ctx: &Ctx,
    subject: Subject,
    selection: String,
    receipt: ShareAgentCreateReceipt,
) -> ApiResult<ShareAgentCreateResponse> {
    let fallback = ShareAgentCreateResponse::Receipt(receipt.clone());
    publication::receipt(ctx, &fallback)?;
    let input = Accepted {
        declaration: row(&selection)?,
        expected: subject,
        receipt,
    };
    match reply::render_completed_replay::<Content>(ctx, ctx.user()?.id(), input).await {
        Ok(Some(full)) => Ok(ShareAgentCreateResponse::full(full)),
        Ok(None) => Ok(fallback),
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(fallback),
        Err(error) => Err(error),
    }
}

pub(in crate::routes::share) fn binding()
-> ObservedCreation<(), ShareWithAgents, ShareAgentCreateResponse> {
    ObservedCreation {
        kind: orch_engine::sharing::creation::KIND,
        replay: |ctx, (), _body: ShareWithAgents, created| {
            Box::pin(async move {
                let selection = created.data["selection"]
                    .as_str()
                    .ok_or_else(|| {
                        ApiError::internal("the accepted share lost its native selection")
                    })?
                    .to_owned();
                let shares: Vec<AgentShareReceipt> =
                    serde_json::from_value(created.data["shares"].clone())
                        .map_err(ApiError::internal)?;
                let subject: Subject = serde_json::from_value(created.data["subject"].clone())
                    .map_err(ApiError::internal)?;
                if created.kind != orch_engine::sharing::creation::KIND
                    || shares.first().map(|share| share.id) != Some(created.id)
                {
                    return Err(ApiError::internal("the accepted share identities changed"));
                }
                render(
                    &ctx,
                    subject,
                    selection,
                    ShareAgentCreateReceipt {
                        projection: orch_api_contract::common::ReceiptProjection::Receipt,
                        shares,
                    },
                )
                .await
            })
        },
    }
}
