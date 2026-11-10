//! Request-local native lifecycle suppliers; no IO is replayed by an effect resolver.
use super::*;
use crate::{harnesses::preparation as startup, jobs::measurement};
use futures::future::BoxFuture;
use orch_access_contract::{decisions::ObjectRef, permissions::Permission};
use orch_types::Json;
use std::{future::Future, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Scope {
    origin: Arc<Mutex<Origin>>,
    saved: Arc<Vec<orch_api_contract::forks::Measurement>>,
}
tokio::task_local! {static ACTIVE:Scope;}
impl Scope {
    pub fn new(origin: Origin) -> Self {
        Self {
            origin: Arc::new(Mutex::new(origin)),
            saved: Arc::new(Vec::new()),
        }
    }
    /// Typed references read from the original saved native files plan, never a request body.
    pub fn with_saved_files(mut self, files: &orch_api_contract::forks::FilesPlan) -> Self {
        let mut references = self.saved.as_ref().clone();
        references.extend(files.home.as_ref().and_then(|home| home.measurement));
        references.extend(
            files
                .file_states
                .as_ref()
                .and_then(|states| states.measurement),
        );
        references.extend(
            files
                .private_views
                .iter()
                .filter_map(|view| view.measurement),
        );
        references.sort_by_key(|reference| reference.job_id);
        references.dedup_by_key(|reference| reference.job_id);
        self.saved = Arc::new(references);
        self
    }
    pub async fn origin(&self) -> Origin {
        self.origin.lock().await.clone()
    }
    async fn retain(&self, origin: &Origin) -> Result<(), EngineError> {
        let mut current = self.origin.lock().await;
        *current = current.accepted(origin.captured().clone(), origin.until())?;
        Ok(())
    }
    pub async fn current(&self, engine: &Engine) -> Result<Caller, EngineError> {
        let origin = self.origin().await;
        let mut tx = engine.db().begin().await?;
        let check = origin
            .checkpoint(&mut tx, engine.inner.clock.as_ref())
            .await?;
        let caller = check.caller().clone();
        let accepted = origin.accepted(
            CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?,
            check.until(),
        )?;
        let deadline = check.deadline();
        check.check(engine.inner.clock.as_ref())?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        deadline.check(engine.inner.clock.as_ref())?;
        self.retain(&accepted).await?;
        Ok(caller)
    }
}
pub async fn scope<T>(source: &Scope, future: impl Future<Output = T>) -> T {
    ACTIVE.scope(source.clone(), future).await
}
pub(crate) fn is_active() -> bool {
    ACTIVE.try_with(|_| ()).is_ok()
}
fn active() -> Option<Scope> {
    ACTIVE.try_with(Clone::clone).ok()
}
/// Native selectors call this with their real chosen use operands, before supplier IO.
pub(crate) async fn select_destinations(
    conn: &mut Conn,
    needs: &[(ObjectRef, Permission)],
) -> Result<(), EngineError> {
    if let Some(scope) = active() {
        scope
            .origin
            .lock()
            .await
            .select_destinations(conn, needs)
            .await?;
    }
    Ok(())
}
pub(crate) async fn checkpoint(engine: &Engine) -> Result<(), EngineError> {
    if let Some(source) = active() {
        source.current(engine).await?;
    }
    Ok(())
}
#[derive(Clone)]
struct Supplier {
    origin: Origin,
    request: measurement::Request,
}
impl Supplier {
    async fn physical(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<bool, EngineError> {
        if self.request.by != self.origin.captured().user() {
            return Ok(false);
        }
        if self.request.kind == orch_link_contract::commands::CommandKind::NodeMountFingerprint {
            let pair: measurement::FingerprintInput =
                serde_json::from_value(self.request.body.clone())?;
            Ok(self
                .origin
                .physical(
                    conn,
                    self.request.machine,
                    self.request.kind,
                    &serde_json::to_value(pair.source)?,
                    clock.time(),
                )
                .await?
                && self
                    .origin
                    .physical(
                        conn,
                        pair.target_machine,
                        self.request.kind,
                        &serde_json::to_value(pair.target)?,
                        clock.time(),
                    )
                    .await?)
        } else {
            self.origin
                .physical(
                    conn,
                    self.request.machine,
                    self.request.kind,
                    &self.request.body,
                    clock.time(),
                )
                .await
        }
    }
    async fn target(&self, conn: &mut Conn, target: startup::Target) -> Result<bool, EngineError> {
        let startup::Target::Measurement {
            job,
            machine,
            kind,
            phase,
            ..
        } = target
        else {
            return Ok(false);
        };
        if kind != self.request.kind {
            return Ok(false);
        }
        let expected = if phase == Some(startup::MeasurementPhase::FingerprintTarget) {
            serde_json::from_value::<measurement::FingerprintInput>(self.request.body.clone())?
                .target_machine
        } else {
            self.request.machine
        };
        if machine != expected {
            return Ok(false);
        }
        if let Some(id) = job {
            let Some(row) = orch_db::queries::jobs::job(conn, id).await? else {
                return Ok(false);
            };
            if row.kind != measurement::KIND
                || row.created_by != Some(self.request.by)
                || row.plan["machine"] != serde_json::json!(self.request.machine)
                || row.plan["command_kind"] != serde_json::json!(kind.as_str())
                || row.plan["body"] != self.request.body
                || row.plan["subject"] != self.request.subject
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
impl startup::Source for Supplier {
    fn origin(&self) -> &CapturedOperation {
        self.origin.captured()
    }
    fn until(&self) -> Option<Time> {
        self.origin.until()
    }
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: startup::Target,
    ) -> BoxFuture<'a, Result<startup::Checkpoint, EngineError>> {
        Box::pin(async move {
            if !self.target(conn, target).await? || !self.physical(conn, clock).await? {
                return Err(closed());
            }
            self.origin.checkpoint(conn, clock).await
        })
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn startup::Source>, EngineError> {
        Ok(Arc::new(Self {
            origin: self.origin.accepted(origin, until)?,
            request: self.request.clone(),
        }))
    }
    fn command<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: startup::Target,
        outgoing: &'a Outgoing,
    ) -> BoxFuture<'a, Result<CommandAuthority, EngineError>> {
        Box::pin(async move {
            if !self.target(conn, target).await? || !self.physical(conn, clock).await? {
                return Err(closed());
            }
            let evidence = NativeCommand::new(conn, &self.origin, clock, target, outgoing).await?;
            Ok(CommandAuthority::lifecycle_preparation(
                CommandEvidence::Native(Box::new(evidence)),
            ))
        })
    }
}
/// Modern native sources always use the shared source-aware supplier; refusals never fall back.
/// Existing ordinary/permission-key counts retain their native cache identity after the current source check.
pub(crate) async fn measure(
    engine: &Engine,
    request: measurement::Request,
) -> Result<Option<measurement::Count>, EngineError> {
    let Some(scope) = active() else {
        return Ok(None);
    };
    // The existing ordinary/permission-key supplier keeps its cache and accepted-job semantics.
    // A known modern source is never allowed to reach that path after a restoration failure.
    let caller = scope.current(engine).await?;
    if !caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        return Ok(None);
    }
    let origin = scope.origin().await;
    let association = crate::jobs::command_identity::hash(&serde_json::to_value(&origin)?)?;
    let source = measurement::Preparation::new(
        association,
        startup::Preparation::new(Supplier {
            origin,
            request: request.clone(),
        }),
    );
    let mut saved = None;
    for reference in scope.saved.iter() {
        if let Some(observation) = engine
            .measurement_reference_observation(&request, reference, &source)
            .await?
        {
            saved = Some(observation.replay(&request, &source)?);
            break;
        }
    }
    let count = if let Some(count) = saved {
        count
    } else {
        // These are the typed native volume references of the reviewed plan. A missing
        // or changed proof must not become a new claim under the confirmation operation.
        if !scope.saved.is_empty()
            && matches!(
                request.kind,
                orch_link_contract::commands::CommandKind::NodeRuntimeVolumesMeasure
                    | orch_link_contract::commands::CommandKind::FsDataMeasure
            )
        {
            return Err(closed());
        }
        engine.measure_with_preparation(request, &source).await?
    };
    if let Some((origin, until)) = count.source() {
        let current = scope.origin().await.accepted(origin.clone(), until)?;
        scope.retain(&current).await?;
    }
    Ok(Some(count))
}
/// The actual native probe is sealed once, before issue, and checked again by the shared command path.
pub(crate) async fn command(
    engine: &Engine,
    mut outgoing: Outgoing,
) -> Result<Outgoing, EngineError> {
    let Some(scope) = active() else {
        return Ok(outgoing);
    };
    let origin = scope.origin().await;
    let mut tx = engine.db().begin().await?;
    if outgoing.deadline.is_none() {
        outgoing.deadline = Some(Time::new(
            engine.inner.clock.now()
                + chrono::Duration::from_std(engine.inner.config.hosting.attach_wait)
                    .map_err(EngineError::unreadable)?,
        ));
    }
    let target = startup::Target::Measurement {
        job: None,
        machine: outgoing.machine,
        kind: outgoing.kind,
        identity: crate::jobs::command_identity::hash(&outgoing.body)?,
        phase: None,
    };
    let native = NativeCommand::new(
        &mut tx,
        &origin,
        engine.inner.clock.as_ref(),
        target,
        &outgoing,
    )
    .await?;
    let accepted = native.origin.clone();
    outgoing.authority = Some(CommandAuthority::lifecycle_preparation(
        CommandEvidence::Native(Box::new(native)),
    ));
    tx.commit().await.map_err(orch_db::DbError::from)?;
    scope.retain(&accepted).await?;
    Ok(outgoing)
}

/// Existing native count semantics, using the same supplier when the lifecycle source is active.
#[expect(
    clippy::too_many_arguments,
    reason = "shared measurement entry keeps wire operands and the optional retained source separate across volume, mount and machine callers"
)]
pub(crate) async fn count(
    engine: &Engine,
    machine: Id,
    kind: orch_link_contract::commands::CommandKind,
    body: Json,
    subject: Json,
    by: Id,
    optional_home: bool,
    fork: Option<&crate::fork_plan::operation::Preparation>,
) -> Result<measurement::Count, EngineError> {
    let request = measurement::Request {
        machine,
        kind,
        body: body.clone(),
        subject: subject.clone(),
        by,
        reuse_unsupported: optional_home,
    };
    if let Some(count) = measure(engine, request).await? {
        return Ok(count);
    }
    let result = if optional_home {
        engine
            .measure_home_in_background(machine, kind, body, subject, by)
            .await
    } else {
        engine
            .measure_in_background_from(machine, kind, body, subject, by, fork)
            .await
    };
    checkpoint(engine).await?;
    result
}
