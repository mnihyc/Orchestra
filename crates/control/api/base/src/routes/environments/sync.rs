//! Native environment-feed members prepare typed sources before their one renderer.

use super::jobs;

use super::read_sources;

pub use crate::key_operations::native;

pub use crate::key_operations::targets;

use crate::middleware::authorization;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::syncs;

use crate::syncs::Piece;

pub use crate::syncs::Reading;

use async_trait::async_trait;

use futures::future::BoxFuture;

pub use orch_access_contract::decisions::ObjectRef;

pub use orch_access_contract::key_scope::KeyTargetKind;

pub use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

pub use orch_api_contract::environment::EnvironmentSyncGetQuery;

use orch_api_contract::environment::EnvironmentSyncGetReply;

pub use orch_api_contract::environment_sources::GetSource as Source;

pub use orch_api_contract::environment_sources::SYNC;

use orch_api_contract::nodes::DeletedInspectionQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::resource_facts::FactSubject;

pub use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_db::queries::feeds;

pub use orch_db::queries::jobs as job_rows;

use orch_engine::resource_facts;

use orch_engine::resource_facts::PreparedVisibility;

use orch_engine::resource_facts::RawFacts;

use orch_engine::resource_facts::library::RecordedUses;

use orch_types::Id;

use orch_types::Seq;

use orch_types::Time;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::collections::HashSet;
/// Native feed window; an overflow returns no jobs and marks the feed incomplete.
pub const JOBS_MAX: i64 = 200;

// The sole native member list supplies selection, rendering and publication refresh.
pub const MEMBERS: &[&dyn NativeMember] = &[&TheEnvironment, &TheLibrary, &TheJobs];

/// Realtime already owns its authenticated snapshot; use the HTTP factory and renderer on it.
pub async fn put_together(reading: &mut Reading<'_>) -> ApiResult<EnvironmentSyncGetReply> {
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
pub trait NativeMember: Sync {
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

pub enum Part {
    Environment(Box<read_sources::Prepared>),
    Library(Option<Box<Library>>),
    Jobs(FeedJobs),
}

impl Part {
    pub async fn render(&self, reading: &mut Reading<'_>) -> ApiResult<Piece> {
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
    pub fn preserves(&self, fresh: &Self) -> bool {
        match (self, fresh) {
            (Self::Environment(old), Self::Environment(now)) => old.preserves(now),
            (Self::Library(None), Self::Library(_)) => true,
            (Self::Library(Some(old)), Self::Library(Some(now))) => old.preserves(now),
            (Self::Jobs(old), Self::Jobs(now)) => old.preserves(now),
            _ => false,
        }
    }
}

pub struct TheEnvironment;

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

pub struct Library {
    pub raw: Vec<RawFacts>,
    pub visibility: PreparedVisibility,
}

impl Library {
    pub fn preserves(&self, fresh: &Self) -> bool {
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

pub struct TheLibrary;

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

pub struct FeedItem {
    pub row: job_rows::JobRow,
    pub source_reference: bool,
}

pub enum JobRows {
    Progress(Vec<job_rows::JobRow>),
    Full(Vec<FeedItem>),
    Cut,
}

pub struct FeedJobs {
    pub rows: JobRows,
    pub names: HashMap<Id, String>,
    pub bounded: bool,
}

impl FeedJobs {
    pub async fn render(&self, conn: &mut Conn) -> ApiResult<Piece> {
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
    pub fn preserves(&self, fresh: &Self) -> bool {
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

orch_api_base::api_family_callback! {
    bind __api_bind_60d76202f7, __api_install_60d76202f7, __api_callback_60d76202f7;
    slot __ApiCallback60d76202f7, __API_CALLBACK_60D76202F7, __api_set_60d76202f7;
    base orch_api_base::routes::environments::sync;

    fn same_job<'__api>(arg0: &'__api orch_db::queries::jobs::JobRow, arg1: &'__api orch_db::queries::jobs::JobRow) -> bool;
}

pub async fn source_reference(
    reading: &mut Reading<'_>,
    row: &job_rows::JobRow,
) -> ApiResult<bool> {
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

pub async fn job_names(
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

pub struct TheJobs;

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

orch_api_base::api_family_callback! {
    bind __api_bind_4a2e170369, __api_install_4a2e170369, __api_callback_4a2e170369;
    slot __ApiCallback4a2e170369, __API_CALLBACK_4A2E170369, __api_set_4a2e170369;
    base orch_api_base::routes::environments::sync;

    fn fact(arg0: orch_api_contract::environment_sources::GetSource, arg1: orch_access_contract::key_scope::KeyTargetKind, arg2: Vec<orch_access_contract::key_scope::KeyResolvedTarget>) -> orch_engine::operation_scope::sources::Facts<orch_api_contract::environment_sources::GetSource>;
}

pub struct Feed;

pub struct Prepared {
    pub parts: Vec<Part>,
    pub seq: Seq,
    pub since: Option<Seq>,
    pub progress_only: bool,
    pub complete: bool,
}

orch_api_base::api_family_callback! {
    bind __api_bind_4b39fa92cc, __api_install_4b39fa92cc, __api_callback_4b39fa92cc;
    slot __ApiCallback4b39fa92cc, __API_CALLBACK_4B39FA92CC, __api_set_4b39fa92cc;
    base orch_api_base::routes::environments::sync;

    async fn prepare<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Caller, arg3: &'__api orch_api_contract::environment::EnvironmentSyncGetQuery, arg4: Option<&'__api orch_api_base::routes::environments::sync::Prepared>) -> orch_api_base::middleware::requests::ApiResult<(orch_api_base::routes::environments::sync::Prepared, Vec<orch_engine::operation_scope::sources::Facts<orch_api_contract::environment_sources::GetSource>>)>;
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
