//! One factual native source for fork planning, confirmation and preparation suppliers.
use super::*;
use crate::operation_scope::{
    continuing::{self, Error, Source as NativeSource},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    fork_sources::{self as rows, PlanRow, PlanSource as Role},
    operation_sources::Declaration,
    routes::Route,
};
use orch_authz::CapturedOperation;
use orch_types::Clock;
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

pub trait Intent: Send + Sync + 'static {
    const ROW: &'static PlanRow;
    const KIND: Kind;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Preview,
    Confirm,
    BatchPreview,
    BatchConfirm,
    Immediate,
}
pub struct Preview;
pub struct Confirm;
pub struct BatchPreview;
pub struct BatchConfirm;
pub struct Immediate;
impl Intent for Preview {
    const ROW: &'static PlanRow = &rows::PREFLIGHT;
    const KIND: Kind = Kind::Preview;
}
impl Intent for Confirm {
    const ROW: &'static PlanRow = &rows::CONFIRM;
    const KIND: Kind = Kind::Confirm;
}
impl Intent for BatchPreview {
    const ROW: &'static PlanRow = &rows::BATCH_PREFLIGHT_ROW;
    const KIND: Kind = Kind::BatchPreview;
}
impl Intent for BatchConfirm {
    const ROW: &'static PlanRow = &rows::BATCH_CONFIRM_ROW;
    const KIND: Kind = Kind::BatchConfirm;
}
impl Intent for Immediate {
    const ROW: &'static PlanRow = &rows::NOW_ROW;
    const KIND: Kind = Kind::Immediate;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Environment {
    id: Id,
    generation: i32,
    machine: Option<Id>,
    root: String,
}
impl Environment {
    fn of(row: &EnvironmentRow) -> Self {
        Self {
            id: row.id,
            generation: row.generation,
            machine: row.machine_id,
            root: row.workspace_root.clone(),
        }
    }
}
/// Only native source associations. Credentials and the original operation remain separate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    thread: Id,
    view: Id,
    segment: Option<Id>,
    revision: i64,
    topic: Id,
    directory: String,
    source_agent: Id,
    source_provider: Id,
    source_environment: Environment,
    target_agent: Id,
    target_provider: Id,
    target_environment: Option<Environment>,
    target_topic: Id,
    target_directory: String,
    new_home: Option<Id>,
    template: Option<Id>,
    machine: Option<Id>,
    request: ForkRequest,
    history: orch_api_contract::forks::ForkHistory,
    reach: Reach,
}
impl Input {
    /// Captures the native selector's actual choice, never its candidate union or returned JSON.
    pub async fn selected(conn: &mut Conn, prepared: &PreparedFork) -> Result<Self, EngineError> {
        let (new_home, template, machine) = match &prepared.target.landing {
            Landing::New { host, .. } => {
                let new = match &prepared.request.environment {
                    Some(ForkEnvironment::New { new }) => new,
                    _ => {
                        return Err(EngineError::unreadable(
                            "The native landing lost its new-environment request.",
                        ));
                    }
                };
                let home = new.home_node_id.unwrap_or(prepared.target.topic);
                let template = match new.template_id {
                    Some(id) => Some(id),
                    None => {
                        setting::<Id>(conn, SettingsPlace::Node(home), keys::ENV_TEMPLATE_ID)
                            .await?
                    }
                };
                (
                    Some(home),
                    template,
                    host.as_ref().map(|host| host.host.machine_id),
                )
            }
            _ => (None, None, None),
        };
        Ok(Self {
            thread: prepared.source.thread.node_id,
            view: prepared.source.thread.active_continuation_id,
            segment: prepared.source.thread.current_segment_id,
            revision: prepared.source.thread.execution_revision,
            topic: prepared.source.thread.topic_id,
            directory: prepared.source.thread.workspace_dir.clone(),
            source_agent: prepared.source.agent.id,
            source_provider: prepared.source.provider.id,
            source_environment: Environment::of(&prepared.source.environment),
            target_agent: prepared.target.agent.id,
            target_provider: prepared.target.provider.id,
            target_environment: prepared.target_environment().map(Environment::of),
            target_topic: prepared.target.topic,
            target_directory: prepared.target.directory.clone(),
            new_home,
            template,
            machine,
            request: prepared.request.clone(),
            history: prepared.history.clone(),
            reach: prepared.reach,
        })
    }
    pub fn thread(&self) -> Id {
        self.thread
    }
    pub fn request(&self) -> &ForkRequest {
        &self.request
    }
    pub(crate) fn effective_request(&self) -> ForkRequest {
        ForkRequest {
            history: Some(self.history.clone()),
            ..self.request.clone()
        }
    }
    pub fn is_new(&self) -> bool {
        self.request.result == ForkResult::New
    }

    /// Physical membership of an actual native supplier, without another permission rule.
    pub(crate) async fn supplier(
        &self,
        conn: &mut Conn,
        machine: Id,
        kind: orch_link_contract::commands::CommandKind,
        body: &Json,
        now: DateTime<Utc>,
    ) -> Result<bool, EngineError> {
        use orch_link_contract::commands::*;
        let source = self.source_environment.id;
        let target = self
            .target_environment
            .as_ref()
            .map_or(source, |env| env.id);
        match kind {
            CommandKind::FsStat => {
                let ask: FsStat = serde_json::from_value(body.clone())?;
                Ok(ask.environment_id == target
                    && ask.mount.is_none()
                    && ask.path == self.target_directory
                    && self
                        .target_environment
                        .as_ref()
                        .unwrap_or(&self.source_environment)
                        .machine
                        == Some(machine))
            }
            CommandKind::GitStatus => {
                let ask: GitStatus = serde_json::from_value(body.clone())?;
                Ok(ask.environment_id == source
                    && self.source_environment.machine == Some(machine)
                    && ask.watch_id.is_none()
                    && self.request.worktree.as_ref().is_some_and(|choice| {
                        ask.path.as_deref() == Some(choice.repository_dir.as_str())
                    }))
            }
            CommandKind::FsManifest => {
                let ask: FsManifest = serde_json::from_value(body.clone())?;
                let carry = carry_of(&self.request);
                let Some(FileSelection::Selection { include, exclude }) = carry.selection.as_ref()
                else {
                    return Ok(false);
                };
                let qualify = |path: &String| {
                    if self.directory.is_empty() {
                        path.clone()
                    } else {
                        format!("{}/{path}", self.directory)
                    }
                };
                let selection = Selection {
                    include: include.iter().map(qualify).collect(),
                    exclude: exclude
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(qualify)
                        .collect(),
                };
                Ok(ask.environment_id == source
                    && self.source_environment.machine == Some(machine)
                    && ask.selection == selection)
            }
            CommandKind::GitWorktreeCopyPlan => {
                let ask: orch_link_contract::source::GitWorktreeCopyPlan =
                    serde_json::from_value(body.clone())?;
                Ok(ask.environment_id == source
                    && self.source_environment.machine == Some(machine)
                    && ask.directory == self.directory
                    && ask.selection
                        == crate::jobs::fork::snapshot_selection(
                            &self.directory,
                            carry_of(&self.request).selection.as_ref(),
                            None,
                        ))
            }
            CommandKind::NodeVolumeCarryCheck => {
                let ask: NodeVolumeCarryCheck = serde_json::from_value(body.clone())?;
                let from = crate::mounts::location_machine(conn, source).await?;
                let into = if self.new_home.is_some() {
                    self.machine
                } else {
                    crate::mounts::location_machine(conn, target).await?
                };
                if Some(machine) != from && Some(machine) != into {
                    return Ok(false);
                }
                let native = orch_db::queries::resources::mounts::data_driver(conn, machine)
                    .await?
                    .is_some_and(|(driver, _)| driver == ask.driver);
                let template = match self.template {
                    Some(id) if self.machine == Some(machine) => {
                        orch_db::queries::resources::hosting::template(conn, id)
                            .await?
                            .is_some_and(|template| template.driver == ask.driver)
                    }
                    _ => false,
                };
                Ok(native || template)
            }
            CommandKind::NodeMountMeasure | CommandKind::NodeMountFingerprint => {
                let (mount, data, driver) = if kind == CommandKind::NodeMountMeasure {
                    let ask: NodeMountMeasure = serde_json::from_value(body.clone())?;
                    (ask.mount_id, ask.data, ask.driver)
                } else {
                    let ask: NodeMountFingerprint = serde_json::from_value(body.clone())?;
                    (ask.mount_id, ask.data, ask.driver)
                };
                let from = crate::mounts::location_machine(conn, source).await?;
                let into = if self.new_home.is_some() {
                    self.machine
                } else {
                    crate::mounts::location_machine(conn, target).await?
                };
                if Some(machine) != from && Some(machine) != into {
                    return Ok(false);
                }
                let mut mounts = crate::mounts::views(conn, source, now).await?;
                if target != source {
                    mounts.extend(crate::mounts::views(conn, target, now).await?);
                }
                if !mounts.iter().any(|view| view.mount.id == mount) {
                    return Ok(false);
                }
                if !orch_db::queries::resources::mounts::data_driver(conn, machine)
                    .await?
                    .is_some_and(|(actual, _)| actual == driver)
                {
                    return Ok(false);
                }
                let locations =
                    orch_db::queries::resources::mounts::locations(conn, &[mount]).await?;
                Ok(locations
                    .iter()
                    .filter(|row| row.machine_id == machine)
                    .any(|row| {
                        crate::mount_copies::DataRef {
                            kind: row.kind.clone(),
                            source: row.source.clone(),
                            storage: row.storage.clone(),
                        }
                        .link()
                            == data
                    }))
            }
            CommandKind::NodeRuntimeVolumesMeasure => {
                let ask: NodeRuntimeVolumesMeasure = serde_json::from_value(body.clone())?;
                let Some(runtime) =
                    orch_db::queries::environments::hosting::live_runtime(conn, source).await?
                else {
                    return Ok(false);
                };
                let Some(host) =
                    orch_db::queries::resources::hosting::host(conn, runtime.host_id).await?
                else {
                    return Ok(false);
                };
                if ask.runtime_id != runtime.id
                    || machine != host.machine_id
                    || ask.volumes.is_empty()
                {
                    return Ok(false);
                }
                let mounts = crate::mounts::views(conn, source, now).await?;
                let mut seen = Vec::new();
                for selected in &ask.volumes {
                    if seen.contains(&selected.selector) {
                        return Ok(false);
                    }
                    seen.push(selected.selector);
                    use orch_driver_contract::volumes::OwnedVolumeSelector as Selector;
                    if !match selected.selector {
                        Selector::Home => carry_of(&self.request).home,
                        Selector::Private(id) => mounts.iter().any(|view| view.mount.id == id),
                        Selector::FileStates => false,
                    } {
                        return Ok(false);
                    }
                    if runtime
                        .volumes
                        .get(crate::volume_carry::role(selected.selector))
                        != Some(&serde_json::to_value(&selected.volume)?)
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn declaration<I: Intent>(&self) -> Result<&'static Declaration<Role>, Error> {
        if self.is_new() {
            I::ROW.fork
        } else {
            I::ROW.switch
        }
        .ok_or(Error::Refused)
    }
}

pub struct Value {
    declaration: &'static Declaration<Role>,
    facts: Vec<Facts<Role>>,
}
pub struct Work<I>(PhantomData<I>);
fn target_error(error: targets::TargetError) -> Error {
    match error {
        targets::TargetError::Missing => Error::Refused,
        targets::TargetError::Db(error) => error.into(),
        targets::TargetError::Authz(error) => error.into(),
    }
}
async fn actual(
    conn: &mut Conn,
    kind: ObjectKind,
    id: Id,
) -> Result<orch_access_contract::key_scope::KeyResolvedTarget, Error> {
    targets::page_object(conn, ObjectRef { kind, id })
        .await
        .map_err(target_error)
}
async fn environment_current(conn: &mut Conn, expected: &Environment) -> Result<bool, Error> {
    Ok(environments::environment(conn, expected.id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .is_some_and(|row| Environment::of(&row) == *expected))
}
async fn collection(
    conn: &mut Conn,
    source: Role,
    values: &[(ObjectKind, Id)],
) -> Result<Facts<Role>, Error> {
    let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for &(kind, id) in &values.iter().copied().collect::<BTreeSet<_>>() {
        groups
            .entry(KeyTargetKind::Object(kind))
            .or_default()
            .push(actual(conn, kind, id).await?);
    }
    Ok(Facts::collection(source, groups.into_iter().collect()))
}
/// Exact current facts of the native selected operands; all targets are resolved at this checkpoint.
pub async fn facts<I: Intent>(conn: &mut Conn, input: &Input) -> Result<Vec<Facts<Role>>, Error> {
    let thread = threads::thread(conn, input.thread)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.archived_at.is_none())
        .ok_or(Error::Refused)?;
    if thread.active_continuation_id != input.view
        || thread.current_segment_id != input.segment
        || thread.execution_revision != input.revision
        || thread.topic_id != input.topic
        || thread.workspace_dir != input.directory
        || thread.agent_id != Some(input.source_agent)
        || thread.provider_id != Some(input.source_provider)
        || thread.environment_id != Some(input.source_environment.id)
        || !environment_current(conn, &input.source_environment).await?
    {
        return Err(Error::Refused);
    }
    if let Some(target) = &input.target_environment {
        if !environment_current(conn, target).await? {
            return Err(Error::Refused);
        }
    }
    scope_facts::<I>(conn, input).await
}
/// Only the managed act's current factual governors. A retained native job owns member association checks.
pub(crate) async fn scope_facts<I: Intent>(
    conn: &mut Conn,
    input: &Input,
) -> Result<Vec<Facts<Role>>, Error> {
    let declaration = input.declaration::<I>()?;
    let main = if declaration
        .bindings
        .iter()
        .any(|binding| binding.source == Role::Thread)
    {
        Facts::one(
            Role::Thread,
            KeyTargetKind::Object(ObjectKind::Thread),
            actual(conn, ObjectKind::Thread, input.thread).await?,
        )
    } else {
        collection(
            conn,
            Role::Sources,
            &[if input.reach == Reach::Environment {
                (ObjectKind::Environment, input.source_environment.id)
            } else {
                (ObjectKind::Thread, input.thread)
            }],
        )
        .await?
    };
    let copied = match &input.request.access {
        Some(orch_api_contract::access::ThreadAccess::As(as_page))
            if as_page.source != input.thread =>
        {
            vec![(ObjectKind::Thread, as_page.source)]
        }
        _ => Vec::new(),
    };
    Ok(vec![
        main,
        collection(conn, Role::CopiedPages, &copied).await?,
    ])
}
/// Actual native batch candidates need their factual scope before the ordinary selector.
/// This is an effect-free preparation admission, not a retained act or a replacement caller.
pub(crate) async fn batch_preparation_caller<I: Intent>(
    conn: &mut Conn,
    caller: &Caller,
    environment: Option<Id>,
    threads: &[Id],
) -> Result<Caller, EngineError> {
    if !caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        return Ok(caller.clone());
    }
    let objects = match environment {
        Some(id) => vec![(ObjectKind::Environment, id)],
        None => threads.iter().map(|&id| (ObjectKind::Thread, id)).collect(),
    };
    let facts = vec![
        collection(conn, Role::Sources, &objects)
            .await
            .map_err(native_error)?,
        collection(conn, Role::CopiedPages, &[])
            .await
            .map_err(native_error)?,
    ];
    batch_caller::<I>(caller, &facts)
}
fn batch_caller<I: Intent>(caller: &Caller, facts: &[Facts<Role>]) -> Result<Caller, EngineError> {
    crate::operation_scope::sources::bind_required(I::ROW.route, I::ROW.phase_default, facts)
        .and_then(|bound| bound.preparation_caller(caller))
        .map_err(|error| EngineError::BeforeEffect(Box::new(error)))
}
/// Source discovery needs one real covered candidate, not the union of a provider's bindings.
/// Native permission selection then chooses the members, whose complete scope is checked separately.
pub(crate) async fn batch_discovery_caller<I: Intent>(
    conn: &mut Conn,
    caller: &Caller,
    threads: &[Id],
) -> Result<Caller, EngineError> {
    if !caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        return Ok(caller.clone());
    }
    if threads.is_empty() {
        return batch_preparation_caller::<I>(conn, caller, None, &[]).await;
    }
    let graph = orch_authz::engine::Graph::load(conn, threads).await?;
    let mut refusal = None;
    for &thread in threads {
        let Some(site) = graph.site_of(thread) else {
            continue;
        };
        let Some(target) = targets::from_site(&graph, &site) else {
            continue;
        };
        if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
            continue;
        }
        let facts = vec![
            Facts::collection(Role::Sources, vec![(target.object.kind, vec![target])]),
            Facts::collection(Role::CopiedPages, Vec::new()),
        ];
        match batch_caller::<I>(caller, &facts) {
            Ok(caller) => return Ok(caller),
            Err(error @ EngineError::BeforeEffect(_)) => refusal = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(refusal.unwrap_or_else(|| EngineError::BeforeEffect(Box::new(Error::Refused))))
}

/// The native selector's factual serialization footprint, shared with its HTTP preparer.
pub async fn lock(conn: &mut Conn, input: &Input) -> Result<(), Error> {
    lock_all(conn, std::slice::from_ref(input)).await
}
/// The union of actual native member rows, in one stable lock order.
pub async fn lock_all(conn: &mut Conn, inputs: &[Input]) -> Result<(), Error> {
    for thread in inputs
        .iter()
        .map(|input| input.thread)
        .collect::<BTreeSet<_>>()
    {
        threads::lock(conn, thread).await?.ok_or(Error::Refused)?;
    }
    let ids = inputs
        .iter()
        .flat_map(|input| {
            input
                .target_environment
                .as_ref()
                .map(|env| env.id)
                .into_iter()
                .chain([input.source_environment.id])
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM environments WHERE id=ANY($1) ORDER BY id FOR KEY SHARE")
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(())
}
impl<I: Intent> NativeSource for Work<I> {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = I::ROW.route;
    const DECLARATION: &'static Declaration<Role> = I::ROW.phase_default;
    fn declaration(_input: &Input, value: &Value) -> &'static Declaration<Role> {
        value.declaration
    }
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let declaration = if input.is_new() {
                I::ROW.fork_phase
            } else {
                I::ROW.switch_phase
            }
            .ok_or(Error::Refused)?;
            let facts = facts::<I>(conn, input).await?;
            Ok((
                Value {
                    declaration,
                    facts: facts.clone(),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if input.reach == Reach::Environment {
                if !orch_authz::resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: input.source_environment.id,
                    },
                    Permission::EnvironmentManage,
                    orch_authz::Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    return Err(Error::Refused);
                }
            } else if !nodes::check(
                conn,
                principal,
                input.thread,
                Permission::ThreadView,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Input,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { lock(conn, input).await })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        old: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.declaration.operation == fresh.declaration.operation
                && old.facts.len() == fresh.facts.len()
                && old.facts.iter().zip(&fresh.facts).all(|(a, b)| {
                    a.source == b.source && a.cardinality == b.cardinality && a.groups == b.groups
                }))
        })
    }
}
/// The native retained job supplies its own original member/phase checks after this shared scope frame.
struct ActWork<I>(PhantomData<I>);
impl<I: Intent> NativeSource for ActWork<I> {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = I::ROW.route;
    const DECLARATION: &'static Declaration<Role> = I::ROW.phase_default;
    fn declaration(input: &Input, value: &Value) -> &'static Declaration<Role> {
        <Work<I> as NativeSource>::declaration(input, value)
    }
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let declaration = if input.is_new() {
                I::ROW.fork_phase
            } else {
                I::ROW.switch_phase
            }
            .ok_or(Error::Refused)?;
            let facts = scope_facts::<I>(conn, input).await?;
            Ok((
                Value {
                    declaration,
                    facts: facts.clone(),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Input,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            threads::lock(conn, input.thread)
                .await?
                .ok_or(Error::Refused)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        old: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        <Work<I> as NativeSource>::check_opened(conn, principal, input, old, fresh)
    }
}
/// Only a native selector's real accepted caller can create this original-operation source.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub(crate) origin: CapturedOperation,
    pub(crate) input: Input,
    kind: Kind,
    credential: crate::operation_scope::request_origin::Credential,
    until: Option<Time>,
}
impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ForkOrigin([private])")
    }
}
impl Origin {
    pub(crate) fn matches_job(
        &self,
        thread: Id,
        request: &ForkRequest,
        reach: Reach,
        batch: Option<Id>,
    ) -> bool {
        self.input.thread == thread
            && self.input.effective_request() == *request
            && self.input.reach == reach
            && match batch {
                Some(_) => self.kind == Kind::BatchConfirm,
                None => matches!(self.kind, Kind::Confirm | Kind::Immediate),
            }
    }

    pub fn capture<I: Intent>(
        caller: &Caller,
        input: Input,
        credential: crate::operation_scope::request_origin::Credential,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        let declaration = input.declaration::<I>().map_err(native_error)?;
        if caller.admitted_operation() != Some(declaration.operation) || !credential.matches(caller)
        {
            return Err(EngineError::BeforeEffect(Box::new(Error::Refused)));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            input,
            kind: I::KIND,
            credential,
            until,
        })
    }
    pub(crate) fn accepted_from(
        &self,
        caller: &Caller,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(self.origin.operation())
            || !self.credential.matches(caller)
        {
            return Err(native_error(Error::Refused));
        }
        let mut next = self.clone();
        next.origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        next.until = next.until.into_iter().chain(until).min();
        Ok(next)
    }
    pub fn deadline(&self, clock: &dyn Clock, caller: &Caller) -> Result<(), EngineError> {
        if caller.key_deadline_allows(clock) && self.until.is_none_or(|until| clock.time() < until)
        {
            Ok(())
        } else {
            Err(EngineError::BeforeEffect(Box::new(Error::Refused)))
        }
    }
    pub(crate) async fn current_any(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<(Principal, Option<Time>), EngineError> {
        match self.kind {
            Kind::Preview => self.current::<Preview>(conn, clock, access).await,
            Kind::Confirm => self.current::<Confirm>(conn, clock, access).await,
            Kind::BatchPreview => self.current::<BatchPreview>(conn, clock, access).await,
            Kind::BatchConfirm => self.current::<BatchConfirm>(conn, clock, access).await,
            Kind::Immediate => self.current::<Immediate>(conn, clock, access).await,
        }
    }
    pub(crate) fn same_act(&self, current: &Self) -> bool {
        self.input == current.input
            && self.kind == current.kind
            && self.credential == current.credential
            && self.origin.operation() == current.origin.operation()
            && self.origin.user() == current.origin.user()
    }
    pub(crate) fn accepted_snapshot(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        if origin.user() != self.origin.user() || origin.operation() != self.origin.operation() {
            return Err(native_error(Error::Refused));
        }
        Ok(Self {
            origin,
            until: self.until.into_iter().chain(until).min(),
            ..self.clone()
        })
    }
    /// Scope of this actual retained act; its native authorizer supplies all source/destination prerequisites.
    pub(crate) async fn current_act(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        retained: &Self,
    ) -> Result<(Principal, Origin), EngineError> {
        if !self.same_act(retained) {
            return Err(native_error(Error::Refused));
        }
        match self.kind {
            Kind::Confirm => self.act::<Confirm>(conn, clock, retained).await,
            Kind::BatchConfirm => self.act::<BatchConfirm>(conn, clock, retained).await,
            Kind::Immediate => self.act::<Immediate>(conn, clock, retained).await,
            _ => Err(native_error(Error::Refused)),
        }
    }
    async fn act<I: Intent>(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        retained: &Self,
    ) -> Result<(Principal, Origin), EngineError> {
        let (caller, until) = crate::operation_scope::request_origin::current(
            conn,
            clock,
            &retained.origin,
            &retained.credential,
            self.until.into_iter().chain(retained.until).min(),
            self.input
                .declaration::<I>()
                .map_err(native_error)?
                .operation,
        )
        .await
        .map_err(native_error)?;
        let frame = continuing::preparation::prepare_with_caller::<ActWork<I>>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.input.clone(),
        )
        .await
        .map_err(native_error)?;
        frame.check_deadline(clock).map_err(native_error)?;
        let principal = frame.into_parts().principal;
        let accepted = self.accepted_from(principal.caller(), until)?;
        accepted.deadline(clock, principal.caller())?;
        Ok((principal, accepted))
    }
    pub fn captured(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn until(&self) -> Option<Time> {
        self.until
    }
    pub fn input(&self) -> &Input {
        &self.input
    }
    /// Compose the source frame with the same native destination selector on the executor's ports.
    pub async fn current<I: Intent>(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<(Principal, Option<Time>), EngineError> {
        if self.kind != I::KIND {
            return Err(EngineError::BeforeEffect(Box::new(Error::Refused)));
        }
        let (caller, until) = crate::operation_scope::request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            self.input
                .declaration::<I>()
                .map_err(native_error)?
                .operation,
        )
        .await
        .map_err(native_error)?;
        let frame = continuing::preparation::prepare_with_caller::<Work<I>>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.input.clone(),
        )
        .await
        .map_err(native_error)?;
        let prepared = prepare_with_access(
            conn,
            clock,
            access,
            frame.principal().caller(),
            self.input.thread,
            &self.input.request,
            self.input.reach,
            None,
            None,
        )
        .await
        .map_err(|error| match error {
            PlanError::Internal(error) => error,
            other => EngineError::BeforeEffect(Box::new(other)),
        })?;
        let current = Input::selected(conn, &prepared).await?;
        if current != self.input
            || (matches!(I::ROW.destination, rows::NativeDestination::Binding) && current.is_new())
        {
            return Err(EngineError::BeforeEffect(Box::new(
                continuing::Error::Refused,
            )));
        }
        frame.check_deadline(clock).map_err(native_error)?;
        if until.is_some_and(|until| clock.time() >= until) {
            return Err(EngineError::BeforeEffect(Box::new(Error::Refused)));
        }
        self.deadline(clock, frame.principal().caller())?;
        Ok((frame.into_parts().principal, until))
    }
}
fn native_error(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        other => EngineError::BeforeEffect(Box::new(other)),
    }
}

/// Actual native supplier context; its real Engine services are runtime-only, never serialized.
#[derive(Clone)]
pub struct Preparation {
    original: Origin,
    retained: std::sync::Arc<tokio::sync::Mutex<Origin>>,
}
pub(crate) struct Checkpoint {
    pub(crate) origin: Origin,
    caller: Caller,
}
impl Checkpoint {
    pub(crate) fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        self.origin.deadline(clock, &self.caller)
    }
}
impl Preparation {
    pub fn new(origin: Origin) -> Self {
        Self {
            original: origin.clone(),
            retained: std::sync::Arc::new(tokio::sync::Mutex::new(origin)),
        }
    }
    pub fn origin(&self) -> &Origin {
        &self.original
    }
    pub async fn retained(&self) -> Origin {
        self.retained.lock().await.clone()
    }
    pub(crate) async fn accept_on(
        &self,
        engine: &Engine,
        conn: &mut Conn,
    ) -> Result<Checkpoint, EngineError> {
        let mut retained = self.retained.lock().await;
        let (principal, until) = retained
            .current_any(
                conn,
                engine.inner.clock.as_ref(),
                engine.inner.access.as_ref(),
            )
            .await?;
        retained.deadline(engine.inner.clock.as_ref(), principal.caller())?;
        *retained = retained.accepted_from(principal.caller(), until)?;
        Ok(Checkpoint {
            origin: retained.clone(),
            caller: principal.caller().clone(),
        })
    }
    pub(crate) async fn accepted(&self, engine: &Engine) -> Result<Origin, EngineError> {
        let mut tx = engine.db().begin().await?;
        let accepted = self.accept_on(engine, &mut tx).await?;
        accepted.check(engine.inner.clock.as_ref())?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        accepted.check(engine.inner.clock.as_ref())?;
        Ok(accepted.origin)
    }
    pub(crate) async fn check(&self, engine: &Engine) -> Result<(), EngineError> {
        self.accepted(engine).await.map(|_| ())
    }
}
