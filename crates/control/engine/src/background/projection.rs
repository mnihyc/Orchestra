//! Native roster facts are prepared once; the actual transport reader owns their final projection.

use super::*;
use orch_api_contract::background::BackgroundEndReason;
use orch_link_contract::background::NativeBackgroundCommand;

fn end_reason(
    interact: bool,
    supported: bool,
    state: BackgroundState,
    current_target: bool,
) -> Option<BackgroundEndReason> {
    if !interact {
        Some(BackgroundEndReason::InteractRequired)
    } else if !supported {
        Some(BackgroundEndReason::Unsupported)
    } else {
        match state {
            BackgroundState::Offline => Some(BackgroundEndReason::Offline),
            BackgroundState::Unavailable => Some(BackgroundEndReason::Unavailable),
            BackgroundState::Live if !current_target => Some(BackgroundEndReason::NoCurrentTarget),
            BackgroundState::Live => None,
        }
    }
}

/// Effect-free factual preparation. This contains no cached permission or reconstructed credential.
#[derive(Debug, Clone)]
pub struct PreparedProjection {
    thread: Id,
    at: Time,
    sources: Vec<PreparedSource>,
    // Masked recorded facts and native support only; Interact is applied by the final reader.
    wake: orch_api_contract::background::BackgroundWakeStatus,
}

#[derive(Debug, Clone)]
struct PreparedSource {
    source: Source,
    observation: NativeBackground,
    commands: Vec<PreparedCommand>,
}

#[derive(Debug, Clone)]
struct PreparedCommand {
    // Retain the complete typed task, including metadata added by native lifecycle producers.
    native: NativeBackgroundCommand,
    recorded: Option<(Id, Option<Id>)>,
    preview: Option<orch_types::Json>,
    outside_workspace: bool,
}

impl Engine {
    /// Await native source lookups and masking before the transport obtains its final current reader.
    pub async fn prepare_background_projection(
        &self,
        conn: &mut Conn,
        thread: Id,
        capture: &Capture,
    ) -> Result<PreparedProjection, BackgroundError> {
        let row = threads::thread(conn, thread)
            .await?
            .ok_or(BackgroundError::NotFound)?;
        let mut sources = Vec::new();
        for (source, observed) in &capture.sessions {
            let environment = orch_db::queries::environments::thread_engine::environment(
                conn,
                source.environment_id,
            )
            .await?;
            let mut commands = Vec::new();
            for native in &observed.items {
                let recorded: Option<(Id, Option<Id>)> = match native.native_item_ref.as_deref() {
                    Some(reference) => sqlx::query_as("SELECT id, turn_id FROM items WHERE thread_id=$1 AND segment_id=$2 AND native_ref=$3 AND kind='tool_call' ORDER BY seq_created LIMIT 1")
                        .bind(thread).bind(source.segment_id).bind(reference).fetch_optional(&mut *conn).await?,
                    None => None,
                };
                let preview = crate::raw_events::mask_live(
                    conn,
                    &row,
                    Some(source.environment_id),
                    json!({"command":native.command,"cwd":native.cwd}),
                )
                .await?;
                let cwd = preview.as_ref().and_then(|value| value["cwd"].as_str());
                let outside_workspace = match (cwd, &environment) {
                    (Some(path), Some(env)) => {
                        let root =
                            std::path::Path::new(&env.workspace_root).join(&row.workspace_dir);
                        let path = std::path::Path::new(path);
                        path.components()
                            .any(|part| matches!(part, std::path::Component::ParentDir))
                            || (path.is_absolute() && !path.starts_with(root))
                    }
                    (Some(_), None) => true,
                    _ => false,
                };
                commands.push(PreparedCommand {
                    native: native.clone(),
                    recorded,
                    preview,
                    outside_workspace,
                });
            }
            sources.push(PreparedSource {
                source: source.clone(),
                observation: observed.clone(),
                commands,
            });
        }
        let candidates: Vec<_> = sources
            .iter()
            .flat_map(|source| {
                source.commands.iter().filter_map(|command| {
                    command.recorded.map(|(item, _)| {
                        (
                            item,
                            format!("{}:{}", source.source.segment_id, command.native.id),
                        )
                    })
                })
            })
            .collect();
        let wake = crate::background_wake::status(conn, &row, &candidates, true).await?;
        Ok(PreparedProjection {
            thread,
            at: capture.at,
            sources,
            wake,
        })
    }

    /// Render the same prepared native facts with the supplied current, admitted transport reader.
    pub async fn render_background_projection(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        prepared: &PreparedProjection,
        provider_directories: &HashSet<Id>,
    ) -> Result<(ThreadBackground, Vec<Target>), BackgroundError> {
        let thread = prepared.thread;
        permitted(
            nodes::check(
                conn,
                principal,
                thread,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await?,
        )?;
        let interact = nodes::check(
            conn,
            principal,
            thread,
            Permission::ThreadInteract,
            Act::Use,
            Policy::default(),
        )
        .await
        .is_ok_and(|decision| decision.is_allowed());
        let mut items = Vec::new();
        let mut targets = Vec::new();
        let mut total = Some(0_i64);
        let mut state = BackgroundState::Live;
        let mut supported = false;
        let mut live = false;
        let mut reason = None;
        for prepared in &prepared.sources {
            let source = &prepared.source;
            let observed = &prepared.observation;
            supported |= observed.supported;
            live |= observed.state == orch_link_contract::background::BackgroundState::Live;
            let source_state = match observed.state {
                orch_link_contract::background::BackgroundState::Live => BackgroundState::Live,
                orch_link_contract::background::BackgroundState::Offline => {
                    BackgroundState::Offline
                }
                orch_link_contract::background::BackgroundState::Unavailable => {
                    BackgroundState::Unavailable
                }
            };
            if source_state != BackgroundState::Live {
                state = source_state;
            }
            if reason.is_none() {
                reason.clone_from(&observed.reason);
            }
            total = total.zip(observed.total).map(|(sum, count)| sum + count);
            let may_see_provider = provider_directories.contains(&source.provider_id)
                && resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id: source.provider_id,
                    },
                    Permission::ProviderManage,
                    Policy::default(),
                )
                .await
                .is_ok_and(|decision| decision.is_allowed());
            for command in &prepared.commands {
                let native = &command.native;
                let id = format!("{}:{}", source.segment_id, native.id);
                let place: BackgroundPlace =
                    serde_json::from_value(serde_json::to_value(native.place)?)?;
                let redacted = if place == BackgroundPlace::Provider {
                    !may_see_provider
                } else {
                    command.outside_workspace
                };
                let cwd = (!redacted)
                    .then(|| {
                        command
                            .preview
                            .as_ref()
                            .and_then(|value| value["cwd"].as_str())
                            .map(str::to_owned)
                    })
                    .flatten();
                items.push(BackgroundCommand {
                    id: id.clone(),
                    segment_id: source.segment_id,
                    command: command
                        .preview
                        .as_ref()
                        .and_then(|value| value["command"].as_str())
                        .map(str::to_owned),
                    command_truncated: native.command_truncated,
                    cwd,
                    cwd_redacted: redacted,
                    place,
                    since: native.since,
                    turn_id: command.recorded.and_then(|(_, turn)| turn),
                    item_id: command.recorded.map(|(item, _)| item),
                    cpu_percent: native.cpu_percent,
                    memory_bytes: native.memory_bytes,
                    end_reason: if interact && source.machine_id.is_none() {
                        Some(BackgroundEndReason::NoCurrentTarget)
                    } else {
                        end_reason(interact, observed.supported, source_state, true)
                    },
                });
                if let Some(machine) = source.machine_id.filter(|_| {
                    observed.supported
                        && observed.state == orch_link_contract::background::BackgroundState::Live
                }) {
                    targets.push(Target {
                        id,
                        segment_id: source.segment_id,
                        machine_id: machine,
                        native_session_ref: source.native_session_ref.clone(),
                        command_id: native.id.clone(),
                        turn_id: command.recorded.and_then(|(_, turn)| turn),
                        item_id: command.recorded.map(|(item, _)| item),
                    });
                }
            }
        }
        if live {
            state = BackgroundState::Live;
        }
        if prepared.sources.is_empty() {
            state = BackgroundState::Unavailable;
            total = None;
            reason = Some("there is no existing native session to inspect".to_owned());
        }
        let end_reason = if interact && prepared.sources.is_empty() {
            Some(BackgroundEndReason::Unavailable)
        } else if interact
            && prepared
                .sources
                .iter()
                .all(|source| source.source.machine_id.is_none())
        {
            Some(BackgroundEndReason::NoCurrentTarget)
        } else {
            end_reason(interact, supported, state, !targets.is_empty())
        };
        let as_of = (state == BackgroundState::Live
            && prepared
                .sources
                .iter()
                .filter(|source| {
                    source.observation.state
                        == orch_link_contract::background::BackgroundState::Live
                })
                .all(|source| source.observation.as_of.is_some()))
        .then_some(prepared.at);
        let mut wake = prepared.wake.clone();
        for task in &mut wake.tasks {
            task.can_end &= interact
                && task
                    .end_id
                    .as_ref()
                    .is_some_and(|id| targets.iter().any(|target| &target.id == id));
            if !task.can_end {
                task.end_id = None;
            }
        }
        Ok((
            ThreadBackground {
                wake: Some(wake),
                supported,
                as_of,
                state,
                reason,
                items,
                next_cursor: None,
                total,
                list_ref: None,
                expires_at: None,
                can_end: end_reason.is_none(),
                end_reason,
            },
            targets,
        ))
    }
}
