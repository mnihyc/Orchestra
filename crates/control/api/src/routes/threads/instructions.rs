//! What a thread's agent reads as its instructions, source by source, as the reader may see it: what the last
//! session was opened with (kept with its segment when the session opened, `segments.instruction_sources`), and the
//! standing instructions as they stand now, apart.
//!
//! The list is made for the reader before it is answered: a file's path only for whoever may view the files of the
//! environment it lies in, and its environment only for whoever may see that; a file of a person's home with its label
//! alone; a place by name only where the reader may know it. It reads no file, makes no request of a model and
//! changes nothing.

use std::collections::HashMap;

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::environment::EnvironmentRef;
use orch_api_contract::instructions::{
    InstructionEvidence, InstructionSource, InstructionSourceKind, StandingNow, ThreadInstructions,
    ThreadInstructionsGet,
};
use orch_authz::{Policy, Principal, resources};
use orch_db::Conn;
use orch_db::queries::threads as rows;
use orch_types::{Hash, Id};
use serde::Deserialize;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(ThreadInstructionsGet, get);
}

/// A source as the segment keeps it.
#[derive(Debug, Deserialize)]
struct Kept {
    kind: InstructionSourceKind,
    label: String,
    #[serde(default)]
    scope: Option<Id>,
    #[serde(default)]
    environment: Option<Id>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    home: bool,
    evidence: InstructionEvidence,
    #[serde(default)]
    bytes: Option<u64>,
    #[serde(default)]
    loaded_bytes: Option<u64>,
    #[serde(default)]
    cut_bytes: Option<u64>,
    #[serde(default)]
    lines: Option<u64>,
    #[serde(default)]
    loaded_lines: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
    /// For a place's standing instructions: the hash of their words when the session was opened.
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    load_reason: Option<String>,
    #[serde(default)]
    parent_path: Option<String>,
    #[serde(default)]
    line_limit: Option<u64>,
    #[serde(default)]
    byte_limit: Option<u64>,
}

/// Whether the caller holds `permission` on the environment `id`.
async fn holds(conn: &mut Conn, principal: &Principal, id: Id, permission: Permission) -> bool {
    let object = ObjectRef {
        kind: ResourceKind::Environment.into(),
        id,
    };
    matches!(
        resources::check(conn, principal, object, permission, Policy::default()).await,
        Ok(Decision::Allowed { .. })
    )
}

/// What the reader may do with the files of one environment.
#[derive(Debug, Clone, Default)]
struct Reach {
    environment: Option<EnvironmentRef>,
    view_files: bool,
    edit_files: bool,
    workspace_root: Option<String>,
}

async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadInstructions> {
    let thread = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (segment_id, observed_at, kept) =
        match rows::opened_instruction_sources(&mut conn, thread).await? {
            Some((segment, at, sources)) => (
                Some(segment),
                at.map(Into::into),
                serde_json::from_value::<Vec<Kept>>(sources).map_err(ApiError::internal)?,
            ),
            None => (None, None, Vec::new()),
        };
    // The standing instructions as they stand now, and whether they are what the session was opened with.
    let parts = orch_engine::resolved::standing_parts(&mut conn, thread)
        .await
        .map_err(ApiError::internal)?;
    let characters = orch_engine::sessions::standing_characters(&mut conn, thread)
        .await
        .map_err(ApiError::internal)?;
    let now: Vec<(Option<Id>, String)> = parts
        .iter()
        .map(|part| (part.node, Hash::of(part.text.as_bytes()).to_string()))
        .collect();
    let then: Vec<(Option<Id>, String)> = kept
        .iter()
        .filter(|source| source.kind == InstructionSourceKind::Standing)
        .map(|source| (source.scope, source.hash.clone().unwrap_or_default()))
        .collect();
    let pending_next_turn = if segment_id.is_some() {
        now != then
    } else {
        !now.is_empty()
    };
    // The places and environments the sources name, as the reader may see them.
    let scopes: Vec<Id> = kept.iter().filter_map(|source| source.scope).collect();
    let named =
        crate::routes::nodes::view::refs(&mut conn, &principal, &scopes, Policy::default()).await?;
    let mut reach: HashMap<Id, Reach> = HashMap::new();
    for environment in kept.iter().filter_map(|source| source.environment) {
        if reach.contains_key(&environment) {
            continue;
        }
        let mut found = Reach::default();
        if holds(
            &mut conn,
            &principal,
            environment,
            Permission::EnvironmentView,
        )
        .await
        {
            found.environment = crate::routes::resources::view::environment_refs(
                &mut conn,
                &[environment],
                &[],
                orch_types::Time::new(ctx.now),
            )
            .await?
            .into_iter()
            .map(|(_, reference)| reference)
            .next();
        }
        found.view_files = holds(&mut conn, &principal, environment, Permission::FilesView).await;
        if found.view_files {
            found.workspace_root =
                orch_db::queries::environments::thread_engine::environment(&mut conn, environment)
                    .await?
                    .map(|row| row.workspace_root);
        }
        found.edit_files = found.view_files
            && holds(&mut conn, &principal, environment, Permission::FilesManage).await;
        reach.insert(environment, found);
    }
    let sources = kept
        .into_iter()
        .enumerate()
        .map(|(order, source)| {
            let place = source
                .environment
                .and_then(|id| reach.get(&id))
                .cloned()
                .unwrap_or_default();
            let file = matches!(
                source.kind,
                InstructionSourceKind::File | InstructionSourceKind::Memory
            );
            // What Orch gave is read in what the agent was given; a file only where its environment's files may be.
            let readable = if file {
                !source.home && place.view_files
            } else {
                true
            };
            let workspace_path = if readable && file {
                source
                    .path
                    .as_deref()
                    .zip(place.workspace_root.as_deref())
                    .and_then(|(path, root)| {
                        std::path::Path::new(path)
                            .strip_prefix(root)
                            .ok()
                            .filter(|relative| !relative.as_os_str().is_empty())
                            .map(|relative| relative.display().to_string())
                    })
            } else {
                None
            };
            InstructionSource {
                order: u32::try_from(order).unwrap_or(u32::MAX),
                kind: source.kind,
                label: source.label,
                scope: source.scope.and_then(|scope| named.get(&scope).cloned()),
                source_environment: place.environment,
                path: source.path.filter(|_| readable && file),
                workspace_path,
                evidence: source.evidence,
                bytes: source.bytes,
                loaded_bytes: source.loaded_bytes,
                cut_bytes: source.cut_bytes,
                lines: source.lines,
                loaded_lines: source.loaded_lines,
                reason: source.reason,
                load_reason: source.load_reason,
                parent_path: source.parent_path.filter(|_| readable && file),
                line_limit: source.line_limit,
                byte_limit: source.byte_limit,
                readable,
                editable: file && !source.home && place.edit_files,
            }
        })
        .collect();
    let context_revision = match segment_id {
        Some(segment) => {
            let sent = rows::context_sent(&mut conn, segment).await?;
            sent.get("lines")
                .filter(|lines| !lines.is_null())
                .map(|lines| Hash::of(lines.to_string().as_bytes()).to_string())
        }
        None => None,
    };
    Ok(ThreadInstructions {
        segment_id,
        observed_at,
        sources,
        standing: StandingNow {
            characters: u64::try_from(characters).unwrap_or(u64::MAX),
            limit: u64::try_from(orch_engine::sessions::STANDING_LIMIT).unwrap_or(u64::MAX),
            pending_next_turn,
        },
        context_revision,
    })
}
