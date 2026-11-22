//! Exact human file commands reuse the raw HTTP source channel and recheck current native geometry.

use super::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use crate::{EngineError, commands::Outgoing};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    workspace::{
        EnvironmentArchiveEntryGet, EnvironmentArchiveList, EnvironmentFileContentGet,
        EnvironmentFileContentPut, EnvironmentFileDelete, EnvironmentFileFetch,
        EnvironmentFileList, EnvironmentFilePreviewGet, EnvironmentFileRename,
        EnvironmentFileSearch, EnvironmentFileStat, EnvironmentGitDiffGet, EnvironmentGitStatusGet,
        EnvironmentUploadWrite,
    },
    workspace_sources::{self as declarations, FileSource as Role},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        environments::workspace::{self, EnvironmentRow},
        link::{CommandRow, NewCommand},
    },
};
use orch_link_contract::commands::{Command, FsAccess};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCommand {
    origin: CapturedOperation,
    environment: Id,
    generation: i32,
    root: String,
    machine: Id,
    kind: String,
    body: Json,
    access: FsAccess,
    write: bool,
}
impl std::fmt::Debug for FileCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileCommand([private])")
    }
}

fn operands(kind: &str, body: &Json) -> Result<Option<(Id, FsAccess, bool)>, EngineError> {
    let command = Command::decode(kind, body.clone()).map_err(EngineError::unreadable)?;
    Ok(match command {
        Command::FsList(body) => Some((body.environment_id, body.access, false)),
        Command::FsStat(body) => Some((body.environment_id, body.access, false)),
        Command::FsRelaySend(body) => Some((body.environment_id, body.access, false)),
        Command::FsRelayDescribe(body) => Some((body.environment_id, body.access, false)),
        Command::FsRelayArchiveRead(body) => Some((body.environment_id, body.access, false)),
        Command::FsFetch(body) => Some((body.environment_id, body.access, false)),
        Command::FsArchiveList(body) => Some((body.environment_id, body.access, false)),
        Command::FsArchiveRead(body) => Some((body.environment_id, body.access, false)),
        Command::FsSearch(body) => Some((body.environment_id, body.access, false)),
        Command::GitStatus(body) => Some((body.environment_id, body.access, false)),
        Command::GitDiff(body) => Some((body.environment_id, body.access, false)),
        Command::FsWrite(body) => Some((body.environment_id, body.access, true)),
        Command::FsRename(body) => Some((body.environment_id, body.access, true)),
        Command::FsRemove(body) => Some((body.environment_id, body.access, true)),
        _ => None,
    })
}
fn effect(body: &Json) -> Json {
    let mut body = body.clone();
    if let Some(fields) = body.as_object_mut() {
        fields.remove("work");
    }
    body
}

async fn source_facts(
    conn: &mut Conn,
    row: &EnvironmentRow,
    machine: Id,
    declaration: &Declaration<Role>,
) -> Result<Vec<Facts<Role>>, Error> {
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: row.id,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    let mut facts = vec![Facts::one(
        Role::Environment,
        KeyTargetKind::Object(ObjectKind::Environment),
        target,
    )];
    if declaration.operation == "EnvironmentUploadWrite" {
        let machine = targets::machine_identities(conn, &[machine])
            .await
            .map_err(EngineError::unreadable)?
            .remove(&machine)
            .ok_or(Error::Refused)?;
        facts.push(Facts::collection(
            Role::MachineNames,
            vec![(KeyTargetKind::Object(ObjectKind::Machine), vec![machine])],
        ));
    }
    Ok(facts)
}

fn boundary_error(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        other => EngineError::BeforeEffect(Box::new(other)),
    }
}

trait Operation: Send + Sync + 'static {
    const ROUTE: Route;
    const SOURCE: &'static Declaration<Role>;
}
struct Factory<O>(std::marker::PhantomData<O>);
impl<O: Operation> Source for Factory<O> {
    type Role = Role;
    type Input = FileCommand;
    type Value = EnvironmentRow;
    const ROUTE: Route = O::ROUTE;
    const DECLARATION: &'static Declaration<Role> = O::SOURCE;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a FileCommand,
        _: Time,
    ) -> BoxFuture<'a, Result<(EnvironmentRow, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = workspace::environment(conn, input.environment)
                .await?
                .ok_or(Error::Refused)?;
            if row.machine_id != Some(input.machine)
                || row.generation != input.generation
                || row.workspace_root != input.root
            {
                return Err(Error::Refused);
            }
            let facts = source_facts(conn, &row, input.machine, O::SOURCE).await?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a FileCommand,
        row: &'a EnvironmentRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let permission = if input.write {
                Permission::FilesManage
            } else {
                Permission::FilesView
            };
            if !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: row.id,
                },
                permission,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            let current =
                orch_authz::file_reach::effective(conn, principal, row.id, Policy::default())
                    .await?;
            let (held, asked) = if input.write {
                (current.manage, input.access.manage)
            } else {
                (current.view, input.access.view)
            };
            if !asked.is_some_and(|asked| held.is_some_and(|held| held >= asked)) {
                return Err(Error::Refused);
            }
            if input.write
                && (!row.remote_write
                    || orch_db::queries::environment_facts::refuses(
                        conn,
                        row.id,
                        "environment.remote_write",
                    )
                    .await?)
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a FileCommand,
        _: &'a EnvironmentRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a FileCommand,
        a: &'a EnvironmentRow,
        b: &'a EnvironmentRow,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(a.id == b.id
                && a.machine_id == b.machine_id
                && a.generation == b.generation
                && a.workspace_root == b.workspace_root)
        })
    }
}
macro_rules! operations {
    ($($name:ident : $endpoint:ident = $declaration:ident, $($command:literal)|+;)+) => {
        $(struct $name;
        impl Operation for $name { const ROUTE: Route = $endpoint::ROUTE; const SOURCE: &'static Declaration<Role> = &declarations::$declaration; })+
        fn supports(operation: &str, kind: &str) -> bool { match operation { $(stringify!($endpoint) => matches!(kind, $($command)|+),)+ _ => false } }
        fn declaration(operation: &str) -> Option<&'static Declaration<Role>> { match operation { $(stringify!($endpoint) => Some($name::SOURCE),)+ _ => None } }
        impl FileCommand {
            pub async fn current(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
                match self.origin.operation() { $(stringify!($endpoint) => current::<$name>(self, conn, clock).await,)+ _ => Ok(None) }
            }
        }
    };
}
operations! {
    List: EnvironmentFileList = FILE_LIST, "fs.list";
    Stat: EnvironmentFileStat = FILE_STAT, "fs.stat";
    Content: EnvironmentFileContentGet = FILE_CONTENT, "fs.stat" | "fs.relay.describe" | "fs.relay.send";
    Preview: EnvironmentFilePreviewGet = FILE_PREVIEW, "fs.stat" | "fs.fetch" | "fs.relay.describe" | "fs.relay.send";
    Fetch: EnvironmentFileFetch = FILE_FETCH, "fs.stat" | "fs.fetch" | "fs.relay.describe" | "fs.relay.send";
    Write: EnvironmentFileContentPut = FILE_WRITE, "fs.write";
    Upload: EnvironmentUploadWrite = FILE_UPLOAD, "fs.write";
    Rename: EnvironmentFileRename = FILE_RENAME, "fs.rename";
    Delete: EnvironmentFileDelete = FILE_DELETE, "fs.remove";
    Search: EnvironmentFileSearch = FILE_SEARCH, "fs.search";
    Archive: EnvironmentArchiveList = FILE_ARCHIVE, "fs.archive.list";
    ArchiveRead: EnvironmentArchiveEntryGet = FILE_ARCHIVE_READ, "fs.archive.read" | "fs.relay.archive-read";
    GitStatusRead: EnvironmentGitStatusGet = FILE_GIT_STATUS, "git.status";
    GitDiffRead: EnvironmentGitDiffGet = FILE_GIT_DIFF, "git.diff";
}
async fn current<O: Operation>(
    source: &FileCommand,
    conn: &mut Conn,
    clock: &dyn Clock,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    match continuing::prepare::<Factory<O>>(conn, clock, &source.origin, source.clone()).await {
        Ok(frame) => {
            frame
                .check_deadline(clock)
                .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?;
            Ok(Some(vec![(Some(frame.principal().caller().clone()), None)]))
        }
        Err(Error::Refused | Error::Binding(_)) => Ok(None),
        Err(Error::Native(error)) => Err(error),
    }
}
impl FileCommand {
    /// Owner-read services retain their existing read-only roots; they never inherit human machine scope.
    pub fn root_service(kind: &str, body: &Json) -> Result<bool, EngineError> {
        Ok(operands(kind, body)?.is_some_and(|(_, access, write)| {
            !write
                && access.view == Some(orch_access_contract::permissions::FileReach::Roots)
                && access.manage.is_none()
        }))
    }
    pub fn applicable(kind: &str) -> bool {
        matches!(
            kind,
            "fs.list"
                | "fs.stat"
                | "fs.relay.send"
                | "fs.relay.describe"
                | "fs.relay.archive-read"
                | "fs.fetch"
                | "fs.archive.list"
                | "fs.archive.read"
                | "fs.search"
                | "git.status"
                | "git.diff"
                | "fs.write"
                | "fs.rename"
                | "fs.remove"
        )
    }
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        route: Route,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let declaration = declaration(route.name)
            .ok_or_else(|| EngineError::unreadable("the route has no native file declaration"))?;
        if !supports(declaration.operation, command.kind.as_str()) {
            return Err(EngineError::unreadable(
                "the file command changed its actual operation",
            ));
        }
        if caller
            .admitted_operation()
            .is_some_and(|operation| operation != declaration.operation)
        {
            return Err(boundary_error(Error::Refused));
        }
        let (environment, access, write) = operands(command.kind.as_str(), &command.body)?
            .ok_or_else(|| EngineError::unreadable("the command opens no file target"))?;
        let row = workspace::environment(conn, environment)
            .await?
            .ok_or_else(|| EngineError::unreadable("the file environment disappeared"))?;
        if row.machine_id != Some(command.machine) {
            return Err(EngineError::unreadable(
                "the file command changed its machine",
            ));
        }
        // An ordinary HTTP caller reaches this native source without prior typed admission.
        // Bind its actual current Environment through the same source supplier used at queue/send.
        let facts = source_facts(conn, &row, command.machine, declaration)
            .await
            .map_err(boundary_error)?;
        let plan = super::sources::bind_required(route, declaration, &facts)
            .map_err(|error| boundary_error(Error::Binding(error)))?;
        let admitted = plan
            .admit(caller, false)
            .map_err(|error| boundary_error(Error::Binding(error)))?;
        let origin = CapturedOperation::capture(&admitted).map_err(EngineError::unreadable)?;
        Ok(Self {
            origin,
            environment,
            generation: row.generation,
            root: row.workspace_root,
            machine: command.machine,
            kind: command.kind.as_str().into(),
            body: effect(&command.body),
            access,
            write,
        })
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn command_kind(&self) -> &'static str {
        match self.kind.as_str() {
            "fs.list" => "fs.list",
            "fs.stat" => "fs.stat",
            "fs.relay.send" => "fs.relay.send",
            "fs.relay.describe" => "fs.relay.describe",
            "fs.relay.archive-read" => "fs.relay.archive-read",
            "fs.fetch" => "fs.fetch",
            "fs.archive.list" => "fs.archive.list",
            "fs.archive.read" => "fs.archive.read",
            "fs.search" => "fs.search",
            "git.status" => "git.status",
            "git.diff" => "git.diff",
            "fs.write" => "fs.write",
            "fs.rename" => "fs.rename",
            "fs.remove" => "fs.remove",
            _ => "",
        }
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
            .bind(self.environment)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(self.machine)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        Ok(())
    }
    pub async fn issue(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.machine_id != self.machine
            || command.kind != self.kind
            || effect(command.body) != self.body
        {
            return Ok(None);
        }
        self.current(conn, clock).await
    }
    pub async fn stored(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.machine_id != self.machine
            || command.kind != self.kind
            || effect(&command.body) != self.body
        {
            return Ok(None);
        }
        self.current(conn, clock).await
    }
}
