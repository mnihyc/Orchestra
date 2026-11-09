//! Real workspace image reads keep the original execution through native queue and send.
use super::{
    Arguments, CallContext, Engine, EngineError,
    limits::{Callers, Limits},
    source,
};
use crate::commands::{CommandAuthority, Outcome, Outgoing};
use futures::future::BoxFuture;
use orch_db::{
    Conn,
    queries::{environments::workspace as environments, link, threads},
};
use orch_link_contract::commands::{CommandKind, FsFetch, FsStat, SerialKey};
use orch_model_contract::generation::GenerationPart;
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Site {
    environment: Id,
    generation: i32,
    machine: Id,
    root: String,
    workspace: String,
    paths: Vec<String>,
    largest: i64,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    source: source::Source,
    call: CallContext,
    site: Site,
    limits: Limits,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frozen {
    source: source::Source,
    images: Vec<GenerationPart>,
    items: Vec<Id>,
}

tokio::task_local! { static READING: Proof; }
const SLOT: &str = "image_input_material";

/// One actual native stat/fetch, outside the connector body and never a public request DTO.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    proof: Proof,
    command: Id,
    kind: String,
    canonical: Json,
}
impl std::fmt::Debug for Read {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImageInput([private])")
    }
}

pub(crate) fn run<'a>(
    engine: &'a Engine,
    mut command: Outgoing,
) -> BoxFuture<'a, Option<Result<Outcome, EngineError>>> {
    Box::pin(async move {
        let proof = READING.try_with(Clone::clone).ok()?;
        Some(
            async {
                if source::Source::captured()? != proof.source {
                    return Err(EngineError::StaleToolSource);
                }
                command.key = SerialKey::Environment(proof.site.environment);
                let stage = source::hash_json(
                    serde_json::json!({"kind":command.kind.as_str(),"body":command.body}),
                )?;
                command.id = source::resource_id(
                    &format!("image-input:{stage}"),
                    crate::ids::tool_result(proof.call.thread, &proof.call.call_ref),
                );
                let command = source::issue(engine, command).await?;
                Ok(engine.inner.commands.wait(command.id).await?)
            }
            .await,
        )
    })
}

pub(crate) async fn seal(
    engine: &Engine,
    conn: &mut Conn,
    command: &mut Outgoing,
) -> Result<bool, EngineError> {
    let Some(proof) = READING.try_with(Clone::clone).ok() else {
        return Ok(false);
    };
    if source::Source::captured()? != proof.source {
        return Err(EngineError::StaleToolSource);
    }
    if command.authority.is_some()
        || !matches!(command.kind, CommandKind::FsStat | CommandKind::FsFetch)
    {
        return Err(EngineError::ToolCallIdentity);
    }
    let read = Read {
        proof,
        command: command.id,
        kind: command.kind.as_str().into(),
        canonical: crate::jobs::command_identity::canonical(
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &command.body,
        )?,
    };
    if !read.operands(
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
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
        let CommandAuthority::ImageInput(original) = original else {
            return Err(EngineError::ToolCallIdentity);
        };
        if original.proof.source != read.proof.source
            || original.proof.call != read.proof.call
            || original.proof.site != read.proof.site
            || original.command != read.command
            || original.kind != read.kind
            || original.canonical != read.canonical
        {
            return Err(EngineError::ToolCallIdentity);
        }
        command.deadline = Some(Time::new(saved.deadline_at));
        command.authority = Some(CommandAuthority::ImageInput(original));
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
                .min(read.proof.limits.deadline()),
        );
        command.authority = Some(CommandAuthority::ImageInput(Box::new(read)));
    }
    Ok(true)
}

impl Read {
    pub(crate) fn command_kind(&self) -> &'static str {
        match self.kind.as_str() {
            "fs.stat" => "fs.stat",
            "fs.fetch" => "fs.fetch",
            _ => "",
        }
    }
    fn operands(
        &self,
        machine: Id,
        key: Option<&str>,
        kind: &str,
        body: &Json,
    ) -> Result<bool, EngineError> {
        let serial = SerialKey::Environment(self.proof.site.environment).wire();
        if machine != self.proof.site.machine || key != serial.as_deref() {
            return Ok(false);
        }
        let Some(path) = body
            .get("path")
            .and_then(Json::as_str)
            .filter(|path| self.proof.site.paths.iter().any(|allowed| allowed == path))
        else {
            return Ok(false);
        };
        let expected = match kind {
            "fs.stat" => serde_json::to_value(FsStat {
                environment_id: self.proof.site.environment,
                mount: None,
                path: path.into(),
            })?,
            "fs.fetch" => serde_json::to_value(FsFetch {
                environment_id: self.proof.site.environment,
                mount: None,
                path: path.into(),
                expect: None,
                max_bytes: Some(self.proof.site.largest),
            })?,
            _ => return Ok(false),
        };
        Ok(body == &expected)
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let _ = threads::lock(conn, self.proof.call.thread).await?;
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
            .bind(self.proof.site.environment)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(self.proof.site.machine)
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
                command.serial_key,
                command.kind,
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
            .proof
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
        source::check_call(conn, &self.proof.call).await?;
        let Some(thread) = threads::thread(conn, self.proof.call.thread).await? else {
            return Ok(None);
        };
        let Some(environment) =
            environments::environment(conn, self.proof.site.environment).await?
        else {
            return Ok(None);
        };
        if thread.environment_id != Some(environment.id)
            || thread.workspace_dir != self.proof.site.workspace
            || environment.machine_id != Some(self.proof.site.machine)
            || environment.generation != self.proof.site.generation
            || environment.workspace_root != self.proof.site.root
            || !crate::commands::workspace::physical(conn, &environment, self.proof.site.machine)
                .await?
        {
            return Ok(None);
        }
        let largest = orch_settings::get::<i64>(
            conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_MAX_BLOB_BYTES,
        )
        .await?;
        if largest.is_none_or(|largest| self.proof.site.largest > largest) {
            return Ok(None);
        }
        let restored = self
            .proof
            .limits
            .restore(
                conn,
                &self.proof.call,
                self.command,
                Time::new(command.deadline_at),
                clock,
                access,
            )
            .await?;
        restored
            .finish(conn, &self.proof.call, self.command, clock)
            .await
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &link::CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Callers>, EngineError> {
        let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_tool_commands WHERE command_id=$1 AND thread_id=$2 AND turn_id=$3 AND answer_id=$4 AND environment_id=$5 AND queue_provider_id IS NULL)")
            .bind(command.id).bind(self.proof.call.thread).bind(self.proof.call.turn).bind(crate::ids::tool_result(self.proof.call.thread,&self.proof.call.call_ref)).bind(self.proof.site.environment).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !owned {
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

impl Engine {
    pub(super) async fn image_inputs(
        &self,
        call: &CallContext,
        args: &Arguments,
    ) -> Result<Vec<GenerationPart>, EngineError> {
        let mut paths = args.images.clone();
        if let Some(mask) = &args.mask {
            paths.push(mask.clone());
        }
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        if paths
            .iter()
            .any(|path| !super::super::workspace_files::relative_file(path))
        {
            return Err(EngineError::ToolCallIdentity);
        }
        let mut conn = self.db().acquire().await?;
        source::check_call(&mut conn, call).await?;
        let saved: Option<Json> =
            sqlx::query_scalar("SELECT plan->$2 FROM thread_tool_calls WHERE answer_id=$1")
                .bind(crate::ids::tool_result(call.thread, &call.call_ref))
                .bind(SLOT)
                .fetch_one(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        if let Some(saved) = saved {
            let frozen: Frozen = serde_json::from_value(saved)?;
            source::original_callers(
                &mut conn,
                call,
                self.inner.clock.as_ref(),
                self.inner.access.as_ref(),
            )
            .await?;
            if frozen.source != source::Source::captured()?
                || frozen.images.len() != paths.len()
                || frozen.items.len() != paths.len()
            {
                return Err(EngineError::ToolCallIdentity);
            }
            for (index, (part, item)) in frozen.images.iter().zip(&frozen.items).enumerate() {
                let GenerationPart::ImageRef { blob, size, .. } = part else {
                    return Err(EngineError::ToolCallIdentity);
                };
                let held:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items i JOIN blobs b ON b.hash=$4 WHERE i.id=$1 AND i.thread_id=$2 AND i.native_ref=$3 AND i.payload->'tool'->>'kind'='image' AND i.payload->'tool'->>'blob'=$5 AND b.size=$6)")
                    .bind(item).bind(call.thread).bind(format!("orch.image_input:{}:{index}",crate::ids::tool_result(call.thread,&call.call_ref))).bind(blob.as_bytes().as_slice()).bind(blob.to_string()).bind(size).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
                if !held {
                    return Err(EngineError::StaleToolSource);
                }
            }
            return Ok(frozen.images);
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
        let largest = orch_settings::get::<i64>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_MAX_BLOB_BYTES,
        )
        .await?
        .ok_or(EngineError::StaleToolSource)?;
        let proof = Proof {
            source: source::Source::captured()?,
            call: call.clone(),
            site: Site {
                environment: environment.id,
                generation: environment.generation,
                machine: environment.machine_id.ok_or(EngineError::StaleToolSource)?,
                root: environment.workspace_root,
                workspace: thread.workspace_dir.clone(),
                paths: paths
                    .iter()
                    .map(|path| crate::parcels::within(&thread.workspace_dir, path))
                    .collect(),
                largest,
            },
            limits: Limits::capture(self, &mut conn, call).await?,
        };
        drop(conn);
        let files = READING
            .scope(proof, self.read_workspace_files(&thread, &paths, largest))
            .await?
            .map_err(|refusal| EngineError::unreadable(refusal.message))?;
        let mut images = Vec::with_capacity(files.len());
        let mut raw_blobs = Vec::new();
        let raw_input = self.image_raw_text(&call.arguments, &mut raw_blobs).await?;
        for (index, file) in files.iter().enumerate() {
            let name = if index < args.images.len() {
                format!("source-{}", index + 1)
            } else {
                "mask-1".into()
            };
            if file.size <= 0 || !file.media_type.starts_with("image/") {
                return Err(EngineError::unreadable(
                    "Every prepared input must be a nonempty image.",
                ));
            }
            let actual = self.image_blob_bytes(file.blob, file.size).await?;
            let media_type = image::guess_format(&actual)
                .map(|format| format.to_mime_type().to_owned())
                .map_err(EngineError::unreadable)?;
            // The keeper fetches exact command-listed blobs; no public source URL is invented.
            images.push(GenerationPart::ImageRef {
                blob: file.blob,
                size: file.size,
                media_type,
                name,
                url: String::new(),
            });
        }
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        source::original_callers(
            &mut tx,
            call,
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await?;
        let turn = threads::turn(&mut tx, call.turn)
            .await?
            .filter(|turn| turn.thread_id == call.thread)
            .ok_or(EngineError::StaleToolSource)?;
        let mut unit = crate::unit::Unit::new(&mut tx, self.inner.clock.now());
        for (blob, size) in raw_blobs {
            orch_db::queries::storage::register_blob(
                unit.conn,
                &blob,
                size,
                self.inner.clock.now(),
            )
            .await?;
        }
        let mut items = Vec::with_capacity(images.len());
        for (index, part) in images.iter().enumerate() {
            let GenerationPart::ImageRef {
                blob,
                size,
                media_type,
                name,
                ..
            } = part
            else {
                return Err(EngineError::ToolCallIdentity);
            };
            orch_db::queries::storage::register_blob(
                unit.conn,
                blob,
                *size,
                self.inner.clock.now(),
            )
            .await?;
            let native = format!(
                "orch.image_input:{}:{index}",
                crate::ids::tool_result(call.thread, &call.call_ref)
            );
            let payload = serde_json::json!({"title":"Image input","started_at":self.inner.clock.time(),"duration_ms":null,"tool":{"kind":"image","blob":blob,"media_type":media_type,"name":name,"caption":null},"raw":{"name":"image_generation.input","input":raw_input,"output":null,"is_error":false}});
            let item = crate::ingest::add_item(
                &mut unit,
                &crate::ingest::ItemSpec {
                    thread: call.thread,
                    segment: turn.segment_id,
                    turn: Some(call.turn),
                    stream: "main",
                    parent: None,
                    kind: "tool_call",
                    status: "completed",
                    author: None,
                    payload: &payload,
                    native_ref: Some(&native),
                },
            )
            .await?;
            let item = match item {
                Some(item) => item.id,
                None => {
                    sqlx::query_scalar("SELECT id FROM items WHERE segment_id=$1 AND native_ref=$2")
                        .bind(turn.segment_id)
                        .bind(&native)
                        .fetch_one(&mut *unit.conn)
                        .await
                        .map_err(orch_db::DbError::from)?
                }
            };
            items.push(item);
        }
        let frozen = Frozen {
            source: source::Source::captured()?,
            images: images.clone(),
            items,
        };
        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,ARRAY[$2::text],$3) WHERE answer_id=$1 AND NOT(plan ? $2)").bind(crate::ids::tool_result(call.thread,&call.call_ref)).bind(SLOT).bind(serde_json::to_value(frozen)?).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(images)
    }
}
