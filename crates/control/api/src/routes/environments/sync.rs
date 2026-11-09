//! Native environment-feed members prepare typed sources before their one renderer.

use super::{jobs, read_sources, reading};
use crate::{
    key_operations::{native, targets},
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
    },
    registry::{Ctx, OperationPlan, Registry},
    syncs::{self, Piece, Reading},
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission, ResourceKind},
};
use orch_api_contract::{
    environment::{EnvironmentSyncGet, EnvironmentSyncGetQuery, EnvironmentSyncGetReply},
    environment_sources::{GetSource as Source, SYNC},
    nodes::DeletedInspectionQuery,
    operation_sources::Declaration,
    operations::FacetUsage,
    resource_facts::FactSubject,
};
use orch_authz::{Caller, Policy, Principal};
use orch_db::{
    Conn,
    queries::{feeds, jobs as job_rows},
};
use orch_engine::resource_facts::{self, PreparedVisibility, RawFacts, library::RecordedUses};
use orch_types::{Id, Seq, Time};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Native feed window; an overflow returns no jobs and marks the feed incomplete.
pub const JOBS_MAX: i64 = 200;

// The sole native member list supplies selection, rendering and publication refresh.
const MEMBERS: &[&dyn NativeMember] = &[&TheEnvironment, &TheLibrary, &TheJobs];

pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        EnvironmentSyncGet,
        native::selected::<Feed>,
        native::selected_read::<Feed>,
    );
}

/// Realtime already owns its authenticated snapshot; use the HTTP factory and renderer on it.
pub(crate) async fn put_together(reading: &mut Reading<'_>) -> ApiResult<EnvironmentSyncGetReply> {
    let query = EnvironmentSyncGetQuery {
        since: reading.since,
    };
    let caller = reading.principal.caller();
    let (value, facts) = prepare(reading.ctx, reading.conn, caller, &query, None).await?;
    if native::modern(caller) {
        let admitted = native::bind(reading.ctx, &SYNC, &facts)?.admit(caller)?;
        if !admitted.content {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the environment feed sources are no longer readable",
            ));
        }
        let principal = authorization::load_principal(
            reading.conn,
            admitted.caller,
            reading.ctx.app.clock.time(),
        )
        .await?;
        let principal =
            crate::sign_in::second_step::hold(reading.conn, reading.ctx.user()?, principal).await?;
        let ctx = reading
            .ctx
            .clone()
            .with_admitted_caller(principal.caller().clone())?;
        <Feed as native::Selected>::require(&ctx, reading.conn, &principal).await?;
        let reply =
            <Feed as native::Selected>::render(&ctx, reading.conn, &principal, &value).await?;
        if !principal
            .caller()
            .key_deadline_allows(ctx.app.clock.as_ref())
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthInvalidToken,
                "the environment feed key has ended",
            ));
        }
        Ok(reply)
    } else {
        <Feed as native::Selected>::require(reading.ctx, reading.conn, reading.principal).await?;
        <Feed as native::Selected>::render(reading.ctx, reading.conn, reading.principal, &value)
            .await
    }
}

#[async_trait]
trait NativeMember: Sync {
    fn name(&self) -> &'static str;
    async fn prepare(
        &self,
        reading: &mut Reading<'_>,
        caller: &Caller,
        plan: &OperationPlan,
    ) -> ApiResult<(Part, Vec<native::Facts<Source>>)>;
    async fn refresh(
        &self,
        reading: &mut Reading<'_>,
        caller: &Caller,
        plan: &OperationPlan,
        opened: &Part,
    ) -> ApiResult<(Part, Vec<native::Facts<Source>>)> {
        let _ = opened;
        self.prepare(reading, caller, plan).await
    }
}

enum Part {
    Environment(Box<read_sources::Prepared>),
    Library(Option<Box<Library>>),
    Jobs(FeedJobs),
}
impl Part {
    async fn render(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
        match self {
            Self::Environment(value) => Ok(Piece::whole(
                serde_json::to_value(
                    value
                        .render(reading.ctx, reading.conn, reading.principal)
                        .await?,
                )
                .map_err(ApiError::internal)?,
            )),
            Self::Library(value) => {
                let facts = match value {
                    None => None,
                    Some(value) => {
                        Some(value.visibility.project(&value.raw).pop().ok_or_else(|| {
                            ApiError::internal("the environment's Library facts were not read")
                        })?)
                    }
                };
                Ok(Piece::whole(
                    serde_json::to_value(facts).map_err(ApiError::internal)?,
                ))
            }
            Self::Jobs(value) => value.render(reading.conn).await,
        }
    }
    fn preserves(&self, fresh: &Self) -> bool {
        match (self, fresh) {
            (Self::Environment(old), Self::Environment(now)) => old.preserves(now),
            (Self::Library(None), Self::Library(_)) => true,
            (Self::Library(Some(old)), Self::Library(Some(now))) => old.preserves(now),
            (Self::Jobs(old), Self::Jobs(now)) => old.preserves(now),
            _ => false,
        }
    }
}

struct TheEnvironment;
#[async_trait]
impl NativeMember for TheEnvironment {
    fn name(&self) -> &'static str {
        "environment"
    }
    async fn prepare(
        &self,
        reading: &mut Reading<'_>,
        caller: &Caller,
        _plan: &OperationPlan,
    ) -> ApiResult<(Part, Vec<native::Facts<Source>>)> {
        let (value, facts) = read_sources::prepare(
            reading.ctx,
            reading.conn,
            caller,
            &DeletedInspectionQuery::default(),
            &SYNC,
        )
        .await?;
        Ok((Part::Environment(Box::new(value)), facts))
    }
}

struct Library {
    raw: Vec<RawFacts>,
    visibility: PreparedVisibility,
}
impl Library {
    fn preserves(&self, fresh: &Self) -> bool {
        let names = self.visibility.sources().collect::<HashSet<_>>();
        let current = fresh.visibility.sources().collect::<HashSet<_>>();
        names.is_subset(&current)
            && self.raw.iter().all(|old| {
                fresh
                    .raw
                    .iter()
                    .find(|row| row.subject == old.subject)
                    .is_some_and(|now| {
                        old.dependents
                            .iter()
                            .filter(|edge| {
                                edge.visibility
                                    .is_some_and(|object| names.contains(&object))
                            })
                            .all(|edge| {
                                now.dependents.iter().any(|current| {
                                    edge.key == current.key
                                        && edge.visibility == current.visibility
                                        && edge.object == current.object
                                        && edge.relation == current.relation
                                })
                            })
                    })
            })
    }
}
struct TheLibrary;
#[async_trait]
impl NativeMember for TheLibrary {
    fn name(&self) -> &'static str {
        "library"
    }
    async fn prepare(
        &self,
        reading: &mut Reading<'_>,
        caller: &Caller,
        plan: &OperationPlan,
    ) -> ApiResult<(Part, Vec<native::Facts<Source>>)> {
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: reading.subject,
        };
        let readable =
            crate::routes::resources::library::readable(reading.conn, reading.principal, &[object])
                .await?;
        let mut selected = Vec::new();
        let value = if readable.contains(&object) {
            let mut raw = resource_facts::read_with(
                reading.conn,
                &[FactSubject::Resource { object }],
                Time::new(reading.ctx.now),
                Some(&RecordedUses),
            )
            .await
            .map_err(crate::key_operations::snapshot_read::engine_error)?;
            crate::routes::everything::library::select_relations(&mut raw);
            let mut visibility = resource_facts::prepare_projection(
                reading.conn,
                reading.principal,
                Policy::default(),
                &raw,
            )
            .await
            .map_err(crate::key_operations::snapshot_read::engine_error)?;
            if native::modern(caller) {
                let mut allowed = HashSet::new();
                for object in visibility.sources() {
                    // Environment Library selects Binding edges from actual stored Threads.
                    let target = targets::node_identity(reading.conn, object.id).await?;
                    if object.kind != ObjectKind::Thread
                        || target.object.kind != KeyTargetKind::Object(ObjectKind::Thread)
                    {
                        return Err(ApiError::internal(
                            "the native environment binding source changed kind",
                        ));
                    }
                    if native::permits_usage(
                        plan,
                        &SYNC,
                        caller,
                        &Source::SyncLibraryThreads,
                        FacetUsage::ResponseContent,
                        KeyTargetKind::Object(ObjectKind::Thread),
                        &target,
                    )? {
                        allowed.insert((object.kind, object.id));
                        selected.push(target);
                    }
                }
                visibility.restrict_to(&allowed);
            }
            Some(Box::new(Library { raw, visibility }))
        } else {
            None
        };
        Ok((
            Part::Library(value),
            if native::modern(caller) {
                vec![fact(
                    Source::SyncLibraryThreads,
                    KeyTargetKind::Object(ObjectKind::Thread),
                    selected,
                )]
            } else {
                Vec::new()
            },
        ))
    }
}

struct FeedItem {
    row: job_rows::JobRow,
    source_reference: bool,
}
enum JobRows {
    Progress(Vec<job_rows::JobRow>),
    Full(Vec<FeedItem>),
    Cut,
}
struct FeedJobs {
    rows: JobRows,
    names: HashMap<Id, String>,
    bounded: bool,
}
impl FeedJobs {
    async fn render(&self, conn: &mut Conn) -> ApiResult<Piece> {
        let mut shown = Vec::new();
        match &self.rows {
            JobRows::Cut => return Ok(Piece::cut(serde_json::json!([]))),
            JobRows::Progress(rows) => {
                for row in rows {
                    shown.push(
                        serde_json::to_value(super::job_ref(row)).map_err(ApiError::internal)?,
                    );
                }
            }
            JobRows::Full(rows) => {
                for item in rows {
                    shown.push(if item.source_reference {
                        serde_json::to_value(orch_engine::jobs::source::reference(&item.row))
                            .map_err(ApiError::internal)?
                    } else {
                        serde_json::to_value(
                            jobs::view_with_machine_sources(
                                conn,
                                &item.row,
                                self.bounded.then_some(&self.names),
                            )
                            .await?,
                        )
                        .map_err(ApiError::internal)?
                    });
                }
            }
        }
        Ok(Piece::whole(serde_json::Value::Array(shown)))
    }
    fn preserves(&self, fresh: &Self) -> bool {
        if !self.names.keys().all(|id| fresh.names.contains_key(id)) {
            return false;
        }
        match (&self.rows, &fresh.rows) {
            (JobRows::Cut, JobRows::Cut) => true,
            (JobRows::Progress(old), JobRows::Progress(now)) => old
                .iter()
                .all(|old| now.iter().any(|now| same_job(old, now))),
            (JobRows::Full(old), JobRows::Full(now)) => old.iter().all(|old| {
                now.iter().any(|now| {
                    same_job(&old.row, &now.row) && (old.source_reference || !now.source_reference)
                })
            }),
            _ => false,
        }
    }
}
fn same_job(old: &job_rows::JobRow, now: &job_rows::JobRow) -> bool {
    old.id == now.id
        && old.kind == now.kind
        && old.environment_id == now.environment_id
        && old.node_id == now.node_id
        && old.provider_id == now.provider_id
        && old.created_by == now.created_by
        && old.created_at == now.created_at
        && old.batch_id == now.batch_id
}
async fn source_reference(reading: &mut Reading<'_>, row: &job_rows::JobRow) -> ApiResult<bool> {
    if !job_rows::kind::SOURCE.contains(&row.kind.as_str()) {
        return Ok(false);
    }
    match authorization::require_resource(
        reading.conn,
        reading.principal,
        ResourceKind::Environment,
        reading.subject,
        Permission::FilesView,
        Policy::default(),
    )
    .await
    {
        Ok(()) => Ok(false),
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::Internal => Err(error),
        Err(_) => Ok(true),
    }
}
async fn job_names(
    reading: &mut Reading<'_>,
    caller: &Caller,
    plan: &OperationPlan,
    rows: JobRows,
) -> ApiResult<(Part, Vec<native::Facts<Source>>)> {
    let full = match &rows {
        JobRows::Full(rows) => rows
            .iter()
            .filter(|item| !item.source_reference)
            .map(|item| item.row.clone())
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let mut names = HashMap::new();
    for id in full
        .iter()
        .filter_map(jobs::waiting_machine)
        .collect::<BTreeSet<_>>()
    {
        if let Some(row) = orch_db::queries::resources::machine(reading.conn, id).await? {
            names.insert(id, row.name);
        }
    }
    let mut selected = Vec::new();
    if native::modern(caller) {
        let mut allowed = HashSet::new();
        for id in names.keys() {
            let target = match targets::machine_identities(reading.conn, &[*id]).await {
                Ok(mut targets) => targets.remove(id).ok_or_else(ApiError::not_found)?,
                Err(error) if error.problem.status == 404 => continue,
                Err(error) => return Err(error),
            };
            if native::permits_usage(
                plan,
                &SYNC,
                caller,
                &Source::SyncJobMachines,
                FacetUsage::ResponseContent,
                KeyTargetKind::Object(ObjectKind::Machine),
                &target,
            )? {
                allowed.insert(*id);
                selected.push(target);
            }
        }
        names.retain(|id, _| allowed.contains(id));
    }
    Ok((
        Part::Jobs(FeedJobs {
            rows,
            names,
            bounded: native::modern(caller),
        }),
        if native::modern(caller) {
            vec![fact(
                Source::SyncJobMachines,
                KeyTargetKind::Object(ObjectKind::Machine),
                selected,
            )]
        } else {
            Vec::new()
        },
    ))
}
struct TheJobs;
#[async_trait]
impl NativeMember for TheJobs {
    fn name(&self) -> &'static str {
        "jobs"
    }
    async fn prepare(
        &self,
        reading: &mut Reading<'_>,
        caller: &Caller,
        plan: &OperationPlan,
    ) -> ApiResult<(Part, Vec<native::Facts<Source>>)> {
        let rows = if reading.progress_only {
            JobRows::Progress(
                job_rows::open_for(
                    reading.conn,
                    &[reading.subject],
                    &job_rows::kind::ONE_AT_A_TIME,
                )
                .await?
                .into_iter()
                .take(1)
                .collect(),
            )
        } else {
            let rows = match reading.since {
                None => job_rows::feeds::open_of_environment(reading.conn, reading.subject).await?,
                Some(since) => {
                    job_rows::feeds::of_environment_between(
                        reading.conn,
                        reading.subject,
                        since,
                        reading.seq,
                        JOBS_MAX + 1,
                    )
                    .await?
                }
            };
            if reading.since.is_some() && i64::try_from(rows.len()).unwrap_or(i64::MAX) > JOBS_MAX {
                JobRows::Cut
            } else {
                let mut items = Vec::with_capacity(rows.len());
                for row in rows {
                    let source_reference = source_reference(reading, &row).await?;
                    items.push(FeedItem {
                        row,
                        source_reference,
                    });
                }
                JobRows::Full(items)
            }
        };
        job_names(reading, caller, plan, rows).await
    }
    async fn refresh(
        &self,
        reading: &mut Reading<'_>,
        caller: &Caller,
        plan: &OperationPlan,
        opened: &Part,
    ) -> ApiResult<(Part, Vec<native::Facts<Source>>)> {
        let Part::Jobs(old) = opened else {
            return Err(ApiError::internal("the native feed member changed"));
        };
        if matches!(old.rows, JobRows::Cut) {
            return job_names(reading, caller, plan, JobRows::Cut).await;
        }
        if reading.progress_only || matches!(old.rows, JobRows::Progress(_)) {
            return self.prepare(reading, caller, plan).await;
        }
        let JobRows::Full(old) = &old.rows else {
            return Err(ApiError::internal(
                "the saved native job selection changed shape",
            ));
        };
        let mut items = Vec::new();
        for old in old {
            let Some(row) = job_rows::job(reading.conn, old.row.id).await? else {
                continue;
            };
            if row.environment_id != Some(reading.subject) {
                continue;
            }
            let source_reference = source_reference(reading, &row).await?;
            items.push(FeedItem {
                row,
                source_reference,
            });
        }
        job_names(reading, caller, plan, JobRows::Full(items)).await
    }
}
fn fact(
    source: Source,
    kind: KeyTargetKind,
    targets: Vec<KeyResolvedTarget>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(kind, targets)]
        },
    )
}

pub(super) struct Feed;
pub(super) struct Prepared {
    parts: Vec<Part>,
    seq: Seq,
    since: Option<Seq>,
    progress_only: bool,
    complete: bool,
}
async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &EnvironmentSyncGetQuery,
    opened: Option<&Prepared>,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    if query.since.is_some_and(|since| since < 0) {
        return Err(ApiError::invalid(
            "since",
            "is a number of the feed, from 0",
        ));
    }
    let subject = ctx.id("id")?;
    let root = if native::modern(caller) {
        vec![native::Facts::one(
            Source::Environment,
            KeyTargetKind::Object(ObjectKind::Environment),
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: subject,
                },
            )
            .await?,
        )]
    } else {
        Vec::new()
    };
    let plan = native::bind_known(ctx, &SYNC, &root)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let progress_only =
        reading::decide(conn, &principal, subject).await? == reading::Reading::ThroughAThread;
    let seq = feeds::environment_seqs(conn, &[subject])
        .await?
        .pop()
        .map(|(_, seq)| seq)
        .ok_or_else(ApiError::not_found)?;
    let ahead = query.since.is_some_and(|since| since > seq);
    let mut reading = Reading {
        ctx,
        conn,
        principal: &principal,
        progress_only,
        subject,
        seq,
        since: if ahead { Some(seq) } else { query.since },
    };
    let mut parts = Vec::new();
    let mut facts = Vec::new();
    for (index, member) in MEMBERS.iter().enumerate() {
        let (part, more) = match opened {
            None => member.prepare(&mut reading, caller, &plan).await?,
            Some(old) => {
                member
                    .refresh(&mut reading, caller, &plan, &old.parts[index])
                    .await?
            }
        };
        parts.push(part);
        facts.extend(more);
    }
    Ok((
        Prepared {
            parts,
            seq,
            since: query.since,
            progress_only,
            complete: !ahead,
        },
        facts,
    ))
}
impl native::Selected for Feed {
    type Source = Source;
    type Query = EnvironmentSyncGetQuery;
    type Value = Prepared;
    type Reply = EnvironmentSyncGetReply;
    const DECLARATION: &'static Declaration<Source> = &SYNC;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a EnvironmentSyncGetQuery,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, query, None))
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a EnvironmentSyncGetQuery,
        opened: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, query, Some(opened)))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<EnvironmentSyncGetReply>> {
        Box::pin(async move {
            let mut reading = Reading {
                ctx,
                conn,
                principal,
                progress_only: value.progress_only,
                subject: ctx.id("id")?,
                seq: value.seq,
                since: value.since,
            };
            let mut pieces = Vec::new();
            for (member, value) in MEMBERS.iter().zip(&value.parts) {
                pieces.push((member.name(), value.render(&mut reading).await?));
            }
            syncs::assemble(pieces, value.seq, value.since, value.complete)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a EnvironmentSyncGetQuery,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.seq <= fresh.seq
                && (opened.progress_only || !fresh.progress_only)
                && opened.parts.len() == fresh.parts.len()
                && opened
                    .parts
                    .iter()
                    .zip(&fresh.parts)
                    .all(|(old, new)| old.preserves(new)))
        })
    }
}
