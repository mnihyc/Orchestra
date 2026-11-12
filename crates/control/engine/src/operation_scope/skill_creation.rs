//! Native Skill source, exact source commands and current record prerequisites.
use super::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use crate::{
    EngineError,
    commands::Outgoing,
    skills::{
        archive::{self, Limits},
        versions,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission, ResourceKind},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    skill_sources::{self as declarations, Source as Role},
    skills::SkillCreate,
};
use orch_authz::{
    Caller, CapturedOperation, Policy, Principal,
    ownership::{self, Making},
};
use orch_db::{
    Conn, DbError,
    queries::{
        environments::{self, workspace},
        hierarchy, resources,
    },
};
use orch_link_contract::commands::{
    CommandKind, FsFetch, FsList, NodeSkillFetch, Secret, SecretOrText, SerialKey, SkillFetchSource,
};
use orch_types::{Clock, Hash, Id, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Intent {
    New,
    Upload {
        blob: Hash,
    },
    Environment {
        environment: Id,
        path: String,
    },
    Git {
        host: Id,
        repository: String,
        reference: String,
        folder: Option<String>,
    },
    Url {
        host: Id,
        address: Hash,
        private: bool,
    },
}
impl Intent {
    pub fn declaration(&self) -> &'static Declaration<Role> {
        match self {
            Self::New => &declarations::NEW,
            Self::Upload { .. } => &declarations::UPLOAD_ROW,
            Self::Environment { .. } => &declarations::ENVIRONMENT_ROW,
            Self::Git { .. } => &declarations::GIT,
            Self::Url { .. } => &declarations::URL,
        }
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Physical {
    None,
    Environment {
        environment: Id,
        owner: Id,
        home: Id,
        org: Id,
        placement: String,
        machine: Option<Id>,
        generation: i32,
        root: String,
        attached: Option<Time>,
        runtime: Option<Id>,
    },
    Host {
        host: Id,
        machine: Id,
        owner: Option<Id>,
        home: Option<Id>,
        org: Option<Id>,
    },
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    origin: CapturedOperation,
    skill: Id,
    home: Id,
    intent: Intent,
    physical: Physical,
    secret: Option<Id>,
    max_bytes: u64,
    max_files: usize,
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SkillSource([private])")
    }
}
pub struct Selected {
    pub facts: Vec<Facts<Role>>,
    pub limits: Limits,
}
async fn physical(conn: &mut Conn, intent: &Intent) -> Result<Physical, EngineError> {
    match intent {
        Intent::Environment { environment, .. } => {
            let row = workspace::environment(conn, *environment)
                .await?
                .ok_or_else(|| {
                    EngineError::unreadable("the Skill directory environment disappeared")
                })?;
            let attached: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("SELECT attached_session FROM environments WHERE id=$1")
                    .bind(environment)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            let runtime = if row.placement == "managed" {
                sqlx::query_scalar("SELECT r.id FROM runtimes r JOIN environments e ON e.id=r.environment_id WHERE e.id=$1 AND r.kind='environment' AND r.generation=e.generation AND r.template_id=e.template_id ORDER BY r.created_at DESC,r.id DESC LIMIT 1").bind(environment).fetch_optional(&mut *conn).await.map_err(DbError::from)?
            } else {
                None
            };
            Ok(Physical::Environment {
                environment: *environment,
                owner: row.owner_id,
                home: row.home_node_id,
                org: row.org_id,
                placement: row.placement,
                machine: row.machine_id,
                generation: row.generation,
                root: row.workspace_root,
                attached: attached.map(Time::new),
                runtime,
            })
        }
        Intent::Git { host, .. } | Intent::Url { host, .. } => {
            let row = resources::hosting::host(conn, *host)
                .await?
                .ok_or_else(|| EngineError::unreadable("the Skill fetch Host disappeared"))?;
            Ok(Physical::Host {
                host: *host,
                machine: row.machine_id,
                owner: row.owner_id,
                home: row.home_id,
                org: row.org_id,
            })
        }
        Intent::New | Intent::Upload { .. } => Ok(Physical::None),
    }
}
pub async fn facts(
    conn: &mut Conn,
    caller: &Caller,
    home: Id,
    intent: &Intent,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let mut facts = vec![Facts::one(
        Role::Home,
        KeyTargetKind::Object(ObjectKind::Skill),
        targets::node(conn, home)
            .await
            .map_err(EngineError::unreadable)?,
    )];
    match intent {
        Intent::New => {}
        Intent::Upload { .. } => facts.push(Facts::one(
            Role::Upload,
            KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Upload),
            targets::account(conn, caller.user())
                .await
                .map_err(EngineError::unreadable)?,
        )),
        Intent::Environment { environment, .. } => facts.push(Facts::one(
            Role::Environment,
            KeyTargetKind::Object(ObjectKind::Environment),
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: *environment,
                },
            )
            .await
            .map_err(EngineError::unreadable)?,
        )),
        Intent::Git { host, .. } | Intent::Url { host, .. } => facts.push(Facts::one(
            Role::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: *host,
                },
            )
            .await
            .map_err(EngineError::unreadable)?,
        )),
    }
    Ok(facts)
}
async fn require_home(conn: &mut Conn, principal: &Principal, home: Id) -> Result<(), Error> {
    use orch_settings::{Place, get};
    use orch_settings_contract::registry::keys;
    let nodes = hierarchy::with_ancestors(conn, &[home]).await?;
    if !nodes.iter().any(|node| {
        node.id == home
            && matches!(node.kind.as_str(), "org" | "project")
            && node.deleted_at.is_none()
    }) {
        return Err(Error::Refused);
    }
    let members = get::<Vec<String>>(conn, Place::Node(home), keys::RESOURCES_CREATED_BY_MEMBERS)
        .await
        .map_err(EngineError::unreadable)?
        .unwrap_or_default()
        .into_iter()
        .filter_map(|kind| kind.parse().ok())
        .collect::<Vec<_>>();
    let external = get::<bool>(conn, Place::Node(home), keys::SHARING_ALLOW_EXTERNAL)
        .await
        .map_err(EngineError::unreadable)?
        .unwrap_or(true);
    let holders = get::<bool>(conn, Place::Node(home), keys::SHARING_ALLOW_HOLDERS)
        .await
        .map_err(EngineError::unreadable)?
        .unwrap_or(true);
    if !matches!(
        ownership::may_make(
            conn,
            principal,
            ResourceKind::Skill,
            Some(home),
            &members,
            Policy {
                owners_name_outsiders: external,
                holders_share: holders
            }
        )
        .await?,
        Making::Allowed(_)
    ) {
        return Err(Error::Refused);
    }
    Ok(())
}
pub struct Creation;
impl Source for Creation {
    type Role = Role;
    type Input = Input;
    type Value = Selected;
    const ROUTE: Route = SkillCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::NEW;
    fn declaration(input: &Input, _value: &Selected) -> &'static Declaration<Role> {
        input.intent.declaration()
    }
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Selected, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            if physical(conn, &input.intent).await? != input.physical {
                return Err(Error::Refused);
            }
            let facts = facts(conn, caller, input.home, &input.intent).await?;
            let limits = versions::limits_at(conn, input.home).await?;
            let limits = Limits {
                max_bytes: limits.max_bytes.min(input.max_bytes),
                max_files: limits.max_files.min(input.max_files),
            };
            Ok((
                Selected {
                    facts: facts.clone(),
                    limits,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _value: &'a Selected,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            require_home(conn, principal, input.home).await?;
            match &input.intent {
                Intent::Upload { blob } => {
                    if !orch_db::queries::uploads::owns(conn, principal.user(), blob).await? {
                        return Err(Error::Refused);
                    }
                }
                Intent::Environment { environment, .. } => {
                    if !orch_authz::resources::check(
                        conn,
                        principal,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: *environment,
                        },
                        Permission::EnvironmentManage,
                        Policy::default(),
                    )
                    .await?
                    .is_allowed()
                    {
                        return Err(Error::Refused);
                    }
                }
                Intent::Git { host, .. } | Intent::Url { host, .. } => {
                    if !orch_authz::resources::check(
                        conn,
                        principal,
                        ObjectRef {
                            kind: ObjectKind::Host,
                            id: *host,
                        },
                        Permission::HostUse,
                        Policy::default(),
                    )
                    .await?
                    .is_allowed()
                    {
                        return Err(Error::Refused);
                    }
                }
                Intent::New => {}
            }
            if let Some(secret) = input.secret {
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM secrets WHERE id=$1 AND owner_kind='skill' AND owner_id=$2 AND created_by=$3)").bind(secret).bind(input.skill).bind(principal.user()).fetch_one(conn).await.map_err(DbError::from)?;
                if !valid {
                    return Err(Error::Refused);
                }
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Input,
        _value: &'a Selected,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        old: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old.limits == fresh.limits) })
    }
}
impl Input {
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        skill: Id,
        home: Id,
        intent: Intent,
        limits: Limits,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(intent.declaration().operation) {
            return Err(EngineError::unreadable(
                "the Skill source changed its original operation",
            ));
        }
        let physical = physical(conn, &intent).await?;
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            skill,
            home,
            intent,
            physical,
            secret: None,
            max_bytes: limits.max_bytes,
            max_files: limits.max_files,
        })
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn declaration(&self) -> &'static Declaration<Role> {
        self.intent.declaration()
    }
    pub fn skill(&self) -> Id {
        self.skill
    }
    pub fn home(&self) -> Id {
        self.home
    }
    pub fn machine(&self) -> Option<Id> {
        match self.physical {
            Physical::Host { machine, .. } => Some(machine),
            Physical::Environment { machine, .. } => machine,
            Physical::None => None,
        }
    }
    pub fn with_secret(&self, secret: Option<Id>) -> Self {
        Self {
            secret,
            ..self.clone()
        }
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let mut nodes = hierarchy::with_ancestors(conn, &[self.home])
            .await?
            .into_iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        nodes.sort_unstable();
        sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
            .bind(&nodes)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
        match self.physical {
            Physical::Environment {
                environment,
                machine,
                runtime,
                ..
            } => {
                sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                    .bind(environment)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                if let Some(runtime) = runtime {
                    sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
                        .bind(runtime)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                }
                if let Some(machine) = machine {
                    sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                        .bind(machine)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                }
            }
            Physical::Host { host, machine, .. } => {
                sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
                    .bind(host)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                    .bind(machine)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            Physical::None => {}
        }
        if let Some(secret) = self.secret {
            sqlx::query("SELECT id FROM secrets WHERE id=$1 FOR SHARE")
                .bind(secret)
                .fetch_optional(conn)
                .await
                .map_err(DbError::from)?;
        }
        Ok(())
    }
    fn contains(&self, path: &str) -> bool {
        let Intent::Environment { path: root, .. } = &self.intent else {
            return false;
        };
        (path.is_empty() && root.is_empty()
            || archive::normal_path(path).is_some_and(|normal| normal == path))
            && (root.is_empty() || path == root || path.starts_with(&format!("{root}/")))
    }
    pub fn command(
        &self,
        command: &Outgoing,
    ) -> Result<super::http_command_source::CommandEvidence, EngineError> {
        if Some(command.machine) != self.machine() || command.key != SerialKey::None {
            return Err(EngineError::unreadable(
                "the Skill command changed its physical source",
            ));
        }
        let phase = match command.kind {
            CommandKind::FsList => {
                let body: FsList = serde_json::from_value(command.body.clone())?;
                if !matches!(&self.intent,Intent::Environment{environment,..} if *environment==body.environment_id)
                    || body.mount.is_some()
                    || !self.contains(&body.path)
                    || body.watch_id.is_some()
                    || body.watch_instance.is_some()
                {
                    return Err(EngineError::unreadable(
                        "the Skill listing changed its directory source",
                    ));
                }
                Phase::List
            }
            CommandKind::FsFetch => {
                let body: FsFetch = serde_json::from_value(command.body.clone())?;
                if !matches!(&self.intent,Intent::Environment{environment,..} if *environment==body.environment_id)
                    || body.mount.is_some()
                    || !self.contains(&body.path)
                    || body.max_bytes.is_none_or(|limit| {
                        limit < 0 || limit > i64::try_from(self.max_bytes).unwrap_or(i64::MAX)
                    })
                {
                    return Err(EngineError::unreadable(
                        "the Skill file fetch changed its directory source",
                    ));
                }
                Phase::File
            }
            CommandKind::NodeSkillFetch => {
                let body: NodeSkillFetch = serde_json::from_value(command.body.clone())?;
                let valid = match (&self.intent, &body.source) {
                    (
                        Intent::Git {
                            repository,
                            reference,
                            folder,
                            ..
                        },
                        SkillFetchSource::Git {
                            repository: found,
                            reference: revision,
                            folder: sub,
                        },
                    ) => repository == found && reference == revision && folder == sub,
                    (
                        Intent::Url {
                            address, private, ..
                        },
                        SkillFetchSource::Url { url },
                    ) => match url {
                        SecretOrText::Text(text) => {
                            !*private
                                && self.secret.is_none()
                                && Hash::of(text.as_bytes()) == *address
                        }
                        SecretOrText::Secret(Secret::Ref(id)) => {
                            *private && self.secret == Some(*id)
                        }
                        _ => false,
                    },
                    _ => false,
                };
                if !valid
                    || body.max_bytes < 0
                    || u64::try_from(body.max_bytes).unwrap_or(u64::MAX)
                        > (Limits {
                            max_bytes: self.max_bytes,
                            max_files: self.max_files,
                        })
                        .archive_bytes()
                {
                    return Err(EngineError::unreadable(
                        "the Skill archive fetch changed its original source",
                    ));
                }
                Phase::Archive
            }
            _ => {
                return Err(EngineError::unreadable(
                    "the Skill source cannot issue that command",
                ));
            }
        };
        Ok(super::http_command_source::CommandEvidence::Skill(
            CommandSource {
                input: self.clone(),
                phase,
            },
        ))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    List,
    File,
    Archive,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSource {
    input: Input,
    phase: Phase,
}
impl CommandSource {
    pub fn command_kind(&self) -> &'static str {
        match self.phase {
            Phase::List => "fs.list",
            Phase::File => "fs.fetch",
            Phase::Archive => "node.skill.fetch",
        }
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.input.lock_source(conn).await
    }
    pub async fn current_issue(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.serial_key.is_some() {
            return Ok(None);
        }
        let Ok(kind) = command.kind.parse() else {
            return Ok(None);
        };
        let outgoing = Outgoing {
            id: command.id,
            machine: command.machine_id,
            key: SerialKey::None,
            kind,
            body: command.body.clone(),
            deadline: Some(Time::new(command.deadline_at)),
            authority: None,
        };
        if !matches!(self.input.command(&outgoing),Ok(super::http_command_source::CommandEvidence::Skill(expected)) if expected==*self)
        {
            return Ok(None);
        }
        self.current(conn, clock, &outgoing).await
    }
    pub async fn current_stored(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: &Outgoing,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let frame = match continuing::prepare::<Creation>(
            conn,
            clock,
            &self.input.origin,
            self.input.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let max = match self.phase {
            Phase::List => None,
            Phase::File => serde_json::from_value::<FsFetch>(command.body.clone())
                .ok()
                .and_then(|body| body.max_bytes)
                .map(|max| (max, frame.value().limits.max_bytes)),
            Phase::Archive => serde_json::from_value::<NodeSkillFetch>(command.body.clone())
                .ok()
                .map(|body| (body.max_bytes, frame.value().limits.archive_bytes())),
        };
        if max.is_some_and(|(actual, allowed)| {
            actual < 0 || u64::try_from(actual).unwrap_or(u64::MAX) > allowed
        }) {
            return Ok(None);
        }
        match self.input.physical {
            Physical::Environment {
                environment,
                machine,
                ..
            } => {
                let row = workspace::environment(conn, environment)
                    .await?
                    .ok_or_else(|| EngineError::unreadable("the Skill directory disappeared"))?;
                let attached = if row.placement == "connected" {
                    environments::hosting::connected(conn, environment)
                        .await?
                        .is_some_and(|directory| directory.attached())
                } else {
                    row.status == "running"
                };
                let machine = match machine {
                    Some(id) => resources::machine(conn, id).await?,
                    None => None,
                };
                if !attached || machine.is_none_or(|machine| !machine.is_active()) {
                    return Ok(None);
                }
            }
            Physical::Host { machine, .. } => {
                if resources::machine(conn, machine)
                    .await?
                    .is_none_or(|machine| !machine.is_active())
                {
                    return Ok(None);
                }
            }
            Physical::None => return Ok(None),
        }
        frame
            .check_deadline(clock)
            .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?;
        Ok(Some(vec![(Some(frame.principal().caller().clone()), None)]))
    }
}
