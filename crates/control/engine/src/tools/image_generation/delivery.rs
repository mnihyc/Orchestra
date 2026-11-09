//! An owned generated image reaches only the actual calling execution's current workspace.
use super::limits::{Callers, Limits};
use super::{CallContext, Engine, EngineError, Output, load_record, source};
use crate::commands::{CommandAuthority, Outcome, Outgoing};
use orch_db::{
    Conn,
    queries::{environments::workspace as environments, link, threads},
};
use orch_link_contract::commands::{CommandKind, ExpectedHash, FsStat, FsWrite, SerialKey};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Destination {
    environment: Id,
    generation: i32,
    machine: Id,
    root: String,
    workspace: String,
    path: String,
}

/// Constructed only after actual source and retained-image checks, never from tool arguments alone.
pub(crate) struct Proof {
    source: source::Source,
    call: CallContext,
    reference: Id,
    index: usize,
    output: Output,
    destination: Destination,
    limits: Limits,
}

tokio::task_local! { static DELIVERY: Proof; }

pub(crate) async fn scope<T>(proof: Proof, work: impl std::future::Future<Output = T>) -> T {
    DELIVERY.scope(proof, work).await
}

impl Proof {
    pub(crate) async fn prepare(
        engine: &Engine,
        call: &CallContext,
        reference: Id,
        index: usize,
        path: &str,
        output: &Output,
    ) -> Result<Self, EngineError> {
        if !super::super::workspace_files::relative_file(path) {
            return Err(EngineError::ToolCallIdentity);
        }
        let mut conn = engine.db().acquire().await?;
        source::check_call(&mut conn, call).await?;
        let limits = Limits::capture(engine, &mut conn, call).await?;
        let record = load_record(&mut conn, call.thread, reference).await?;
        if record.images.get(index) != Some(output) {
            return Err(EngineError::ToolCallIdentity);
        }
        let thread = threads::thread(&mut conn, call.thread)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let environment = environments::environment(
            &mut conn,
            thread.environment_id.ok_or(EngineError::StaleToolSource)?,
        )
        .await?
        .ok_or(EngineError::StaleToolSource)?;
        let machine = environment.machine_id.ok_or(EngineError::StaleToolSource)?;
        Ok(Self {
            source: source::Source::captured()?,
            call: call.clone(),
            reference,
            index,
            output: output.clone(),
            destination: Destination {
                environment: environment.id,
                generation: environment.generation,
                machine,
                root: environment.workspace_root,
                workspace: thread.workspace_dir.clone(),
                path: crate::parcels::within(&thread.workspace_dir, path),
            },
            limits,
        })
    }
}

/// One closed native output-write proof in the ordinary private command channel.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    source: source::Source,
    call: CallContext,
    reference: Id,
    index: usize,
    output: Output,
    destination: Destination,
    limits: Limits,
    command: Id,
    kind: String,
    canonical: Json,
}

impl std::fmt::Debug for Write {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImageWrite([private])")
    }
}

fn canonical(command: &Outgoing) -> Result<Json, EngineError> {
    crate::jobs::command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
}

/// Runs after the shared producer has assigned its canonical native-stage id.
pub(crate) async fn seal(
    engine: &Engine,
    conn: &mut Conn,
    command: &mut Outgoing,
) -> Result<bool, EngineError> {
    let Some(mut proof) = DELIVERY
        .try_with(|proof| Write {
            source: proof.source,
            call: proof.call.clone(),
            reference: proof.reference,
            index: proof.index,
            output: proof.output.clone(),
            destination: proof.destination.clone(),
            limits: proof.limits.clone(),
            command: command.id,
            kind: command.kind.as_str().into(),
            canonical: Json::Null,
        })
        .ok()
    else {
        return Ok(false);
    };
    if source::Source::captured()? != proof.source {
        return Err(EngineError::StaleToolSource);
    }
    if command.authority.is_some()
        || !matches!(command.kind, CommandKind::FsWrite | CommandKind::FsStat)
    {
        return Err(EngineError::ToolCallIdentity);
    }
    proof.canonical = canonical(command)?;
    if !proof.operands(
        command.machine,
        command.kind.as_str(),
        command.key.wire().as_deref(),
        &command.body,
    )? {
        return Err(EngineError::ToolCallIdentity);
    }
    if let Some(saved) = link::command(conn, command.id).await? {
        let original: CommandAuthority = serde_json::from_value(
            link::command_authority(conn, command.id)
                .await?
                .ok_or(EngineError::ToolCallIdentity)?,
        )?;
        let CommandAuthority::ImageWrite(original) = original else {
            return Err(EngineError::ToolCallIdentity);
        };
        if original.source != proof.source
            || original.call != proof.call
            || original.reference != proof.reference
            || original.index != proof.index
            || original.output != proof.output
            || original.destination != proof.destination
            || original.command != proof.command
            || original.kind != proof.kind
            || original.canonical != proof.canonical
        {
            return Err(EngineError::ToolCallIdentity);
        }
        command.deadline = Some(Time::new(saved.deadline_at));
        command.authority = Some(CommandAuthority::ImageWrite(original));
    } else {
        let latest = Time::new(
            engine.inner.clock.now()
                + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
        );
        command.deadline = Some(
            command
                .deadline
                .unwrap_or(latest)
                .min(latest)
                .min(proof.limits.deadline()),
        );
        command.authority = Some(CommandAuthority::ImageWrite(Box::new(proof)));
    }
    Ok(true)
}

impl Write {
    pub(crate) fn command_kind(&self) -> &'static str {
        match self.kind.as_str() {
            "fs.write" => "fs.write",
            "fs.stat" => "fs.stat",
            _ => "",
        }
    }

    fn operands(
        &self,
        machine: Id,
        kind: &str,
        key: Option<&str>,
        body: &Json,
    ) -> Result<bool, EngineError> {
        let serial = SerialKey::Environment(self.destination.environment).wire();
        if machine != self.destination.machine || key != serial.as_deref() {
            return Ok(false);
        }
        let expected = match kind {
            "fs.write" => serde_json::to_value(FsWrite {
                work: None,
                environment_id: self.destination.environment,
                path: self.destination.path.clone(),
                blob: self.output.blob,
                mode: Some(crate::manifest::DEFAULT_FILE_MODE),
                expected_hash: Some(ExpectedHash::Absent),
            })?,
            "fs.stat" => serde_json::to_value(FsStat {
                environment_id: self.destination.environment,
                mount: None,
                path: self.destination.path.clone(),
            })?,
            _ => return Ok(false),
        };
        Ok(body == &expected)
    }

    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let _ = threads::lock(conn, self.call.thread).await?;
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
            .bind(self.destination.environment)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(self.destination.machine)
            .execute(conn)
            .await
            .map_err(orch_db::DbError::from)?;
        Ok(())
    }

    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Callers>, EngineError> {
        if command.id != self.command
            || command.kind != self.kind
            || !self.operands(
                command.machine_id,
                command.kind,
                command.serial_key,
                command.body,
            )?
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        self.lock_source(conn).await?;
        let checked = self
            .source
            .scope(Box::pin(self.current_source(conn, command, clock, access)))
            .await;
        match checked {
            Err(EngineError::StaleToolSource | EngineError::ToolCallIdentity) => Ok(None),
            result => result,
        }
    }

    async fn current_source(
        &self,
        conn: &mut Conn,
        command: &link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Callers>, EngineError> {
        source::check_call(conn, &self.call).await?;
        let record = load_record(conn, self.call.thread, self.reference).await?;
        if record.images.get(self.index) != Some(&self.output)
            || !super::retained_images(conn, self.call.thread, &record).await?
        {
            return Ok(None);
        }
        let Some(thread) = threads::thread(conn, self.call.thread).await? else {
            return Ok(None);
        };
        let Some(environment) =
            environments::environment(conn, self.destination.environment).await?
        else {
            return Ok(None);
        };
        if thread.environment_id != Some(environment.id)
            || thread.workspace_dir != self.destination.workspace
            || environment.generation != self.destination.generation
            || environment.machine_id != Some(self.destination.machine)
            || environment.workspace_root != self.destination.root
            || !environment.remote_write
            || orch_db::queries::environment_facts::refuses(
                conn,
                environment.id,
                "environment.remote_write",
            )
            .await?
            || !crate::commands::workspace::physical(conn, &environment, self.destination.machine)
                .await?
            || orch_db::queries::jobs::changes::environment_change_hold(conn, environment.id)
                .await?
                .is_some_and(|job| job.kind == orch_db::queries::jobs::kind::WORKSPACE_RESTORE)
        {
            return Ok(None);
        }
        let restored = self
            .limits
            .restore(
                conn,
                &self.call,
                self.command,
                Time::new(command.deadline_at),
                clock,
                access,
            )
            .await?;
        super::result_access(conn, &self.call, &record, &restored.origins, clock).await?;
        let size: Option<i64> = sqlx::query_scalar("SELECT size FROM blobs WHERE hash=$1")
            .bind(self.output.blob.as_bytes().as_slice())
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let largest = orch_settings::get::<i64>(
            conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_MAX_BLOB_BYTES,
        )
        .await?;
        if size != Some(self.output.size)
            || self.output.size < 0
            || largest.is_none_or(|largest| self.output.size > largest)
        {
            return Ok(None);
        }
        restored.finish(conn, &self.call, self.command, clock).await
    }

    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &link::CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Callers>, EngineError> {
        let actual: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_tool_commands WHERE command_id=$1 AND thread_id=$2 AND turn_id=$3 AND answer_id=$4 AND environment_id=$5 AND queue_provider_id IS NULL)")
            .bind(command.id).bind(self.call.thread).bind(self.call.turn).bind(crate::ids::tool_result(self.call.thread,&self.call.call_ref)).bind(self.destination.environment).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !actual {
            return Ok(None);
        }
        self.current_issue_callers(
            conn,
            &link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                serial_key: command.serial_key(),
                body: &command.body,
                deadline_at: command.deadline_at,
            },
            clock,
            access,
        )
        .await
    }
}

/// A conflict is a current, guarded stat of that exact destination, never an unscoped file read.
pub(crate) async fn same_file(
    engine: &Engine,
    machine: Id,
    environment: Id,
    path: &str,
    blob: &Hash,
) -> Option<Result<crate::sharing::FileWritten, EngineError>> {
    let (active, source, answer) = DELIVERY
        .try_with(|proof| {
            (
                proof.destination.machine == machine
                    && proof.destination.environment == environment
                    && proof.destination.path == path
                    && proof.output.blob == *blob,
                proof.source,
                crate::ids::tool_result(proof.call.thread, &proof.call.call_ref),
            )
        })
        .ok()?;
    Some(
        async {
            if !active || source::Source::captured()? != source {
                return Err(EngineError::ToolCallIdentity);
            }
            let stage = format!("image-delivery-stat:{environment}:{path}:{blob}");
            let command = Outgoing::new(
                source::resource_id(&stage, answer),
                machine,
                SerialKey::Environment(environment),
                &FsStat {
                    environment_id: environment,
                    mount: None,
                    path: path.into(),
                },
            )?;
            let command = source::issue(engine, command).await?;
            let there = match engine.inner.commands.wait(command.id).await? {
                Outcome::Succeeded(stat) => stat
                    .get("hash")
                    .cloned()
                    .map(serde_json::from_value::<Hash>)
                    .transpose()?,
                _ => {
                    return Err(EngineError::unreadable(
                        "The current bytes at the conflicting destination could not be confirmed.",
                    ));
                }
            }
            .ok_or_else(|| {
                EngineError::unreadable("The current destination did not report its content hash.")
            })?;
            Ok(if &there == blob {
                crate::sharing::FileWritten::Ready
            } else {
                crate::sharing::FileWritten::Differs
            })
        }
        .await,
    )
}
