//! An environment taken to another host with its identity kept: lock, successor, copy, verify, commit; undone before the commit.
//!
//! A destination remains a candidate while its workspace is copied. The source's threads are held,
//! credentials are brought home before copying, and suspended processes resume on a failure. Work
//! whose session cannot be preserved is shown as a blocker before any unit is made.

use crate::commands::{Outcome, Outgoing};
use crate::ids;
use crate::jobs::{
    provision::Provision,
    recreate::Recreating,
    runner::{Done, Failure, Job, Phase, Phased, Stop, Undone, initial_progress},
};
use crate::manifest::EntryKind;
use crate::unit::Unit;
use crate::updates::{self, Draft, Recipe, Target, UpdateError};
use crate::{Engine, EngineError};
use async_trait::async_trait;
use orch_api_contract::environment::{EnvironmentMovePreflightBody, UpdatePreflightReason};
use orch_api_contract::forks::{
    ChangeOptions, FilesMissing, FilesPlan, FilesSource, MoveThreadAction, PathMap,
};
use orch_authz::Principal;
use orch_db::queries::{
    environments::{changes, hosting as envs, workspace as workspace_rows},
    jobs,
    resources::{hosting as hosts, thread_engine as resources},
    storage, threads,
};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{
    FsManifest, FsManifestResult, FsMaterialize, FsMaterializeResult, FsSnapshot, FsSnapshotResult,
    MaterializePurpose, NodeRuntimeRemove, NodeRuntimeStop, NodeVolumeRemove, ProcResume,
    ProcSuspend, PtyClose, Selection, SerialKey,
};
use orch_types::{Hash, Id, Json, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};

/// Move weights make the workspace transfer the largest part of the progress bar.
pub const PHASES: [Phase; 8] = [
    Phase {
        name: "lock",
        weight: 5,
    },
    Phase {
        name: "provision",
        weight: 15,
    },
    Phase {
        name: "copy",
        weight: 55,
    },
    Phase {
        name: "history",
        weight: 5,
    },
    Phase {
        name: "after_up",
        weight: 5,
    },
    Phase {
        name: "verify",
        weight: 10,
    },
    Phase {
        name: "commit",
        weight: 5,
    },
    Phase {
        name: "cleanup",
        weight: 5,
    },
];
/// Random trailing UUID digits keep names distinct for concurrent jobs.
const NAME_DIGITS: usize = 12;
/// A gibibyte is the unit hosts report for their storage sizes.
const BYTES_PER_GIB: i64 = 1024 * 1024 * 1024;

fn fail(code: &str, message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new(code, message))
}

/// Translates a reviewed selection, refusing a whole live tree before any connector is asked.
pub fn move_selection(
    files: Option<&orch_api_contract::forks::FileSelection>,
    live: bool,
) -> Result<Selection, UpdateError> {
    use orch_api_contract::forks::FileSelection;
    let selection = match files {
        Some(FileSelection::Selection { include, exclude })
            if !include.is_empty() && include.iter().all(|path| !path.is_empty()) =>
        {
            Selection {
                include: include.clone(),
                exclude: exclude.clone().unwrap_or_default(),
            }
        }
        Some(FileSelection::Nothing) => {
            return Err(UpdateError::Blocked(vec![updates::blocker(
                "selection_required",
                "a move needs selected workspace files; choose paths to copy",
            )]));
        }
        _ if live => {
            return Err(UpdateError::Blocked(vec![updates::blocker(
                "selection_required",
                "this workspace is listed live; choose explicit paths before moving it",
            )]));
        }
        Some(FileSelection::Selection { .. }) => {
            return Err(UpdateError::Blocked(vec![updates::blocker(
                "selection_required",
                "a selection names at least one relative path",
            )]));
        }
        _ => Selection::default(),
    };
    for pattern in selection.include.iter().chain(&selection.exclude) {
        orch_link_contract::commands::validate_selection_pattern(pattern).map_err(|message| {
            UpdateError::Blocked(vec![updates::blocker("selection_invalid", message)])
        })?;
    }
    Ok(selection)
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn a_live_workspace_is_moved_only_with_reviewed_relative_paths() {
        use orch_api_contract::forks::FileSelection;
        assert!(move_selection(None, true).is_err());
        assert!(move_selection(Some(&FileSelection::All), true).is_err());
        let chosen = FileSelection::Selection {
            include: vec!["src/**".into()],
            exclude: Some(vec!["src/cache".into()]),
        };
        assert_eq!(
            move_selection(Some(&chosen), true).unwrap().include,
            vec!["src/**"]
        );
        let escape = FileSelection::Selection {
            include: vec!["../other".into()],
            exclude: None,
        };
        assert!(move_selection(Some(&escape), true).is_err());
    }
}

/// Durable destination and source information for a move.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveRecipe {
    /// Borrowed source directories are copied into new owned target storage only after consent.
    #[serde(default)]
    pub copy_directories: bool,
    /// Exact installed-files source promised by the move, rather than silently returning to the base image.
    #[serde(default)]
    pub kept_image: Option<orch_driver_contract::kept::KeptImage>,
    /// The explicit workspace selection promised by the plan.
    #[serde(default)]
    pub selection: Selection,
    /// Common hold and runtime definition.
    #[serde(flatten)]
    pub replacement: Recipe,
    /// The destination host selected by the plan.
    pub destination: Id,
    /// The request's explicit host, or automatic scheduling.
    pub named_host: Option<Id>,
    /// The source connector, when it is live.
    pub source_machine: Option<Id>,
    /// The source state restored by a failed move.
    pub source_status: String,
    /// The workspace list used by a cached-source move.
    pub manifest: Hash,
    /// Whether the source can hold its processes still.
    pub held_still: bool,
    /// File counts and transfer costs promised by the plan.
    pub files: FilesPlan,
    /// The mounts the destination does not attach, and the data copied there before it does.
    #[serde(default)]
    pub mount_decisions: crate::mount_copies::MountDecisions,
}

/// The plan of a move before names and visibility are filled by HTTP.
pub struct MoveDraft {
    /// What is kept or detached on the target, including its actual mode and any reason.
    pub mounts: Vec<orch_api_contract::forks::PlanMount>,
    /// What becomes of each MCP server there.
    pub mcp: Vec<orch_api_contract::forks::PlanMcpServer>,
    /// The executable recipe.
    pub recipe: MoveRecipe,
    /// The chosen place with its warnings and resource-use checks.
    pub destination: crate::places::HostPlace,
    /// What each affected thread can do on the target.
    pub threads: Vec<(Id, MoveThreadAction)>,
    /// What prevents this plan from being confirmed.
    pub blockers: Vec<orch_api_contract::forks::PlanBlocker>,
}

impl Engine {
    /// Plans a workspace move without taking a hold or starting a unit.
    pub async fn draft_move(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        environment: Id,
        request: &EnvironmentMovePreflightBody,
        given: Option<&ChangeOptions>,
    ) -> Result<MoveDraft, UpdateError> {
        let target = Target::Environment(environment);
        let common = self
            .draft_update(
                conn,
                principal,
                target,
                UpdatePreflightReason::Recreate,
                given,
            )
            .await?;
        let state = envs::environment(conn, environment)
            .await?
            .ok_or(UpdateError::NotFound)?;
        if request
            .template_id
            .is_some_and(|template| Some(template) != state.template_id)
        {
            return Err(UpdateError::Invalid("A move keeps the current unit template. Change its template through an update before moving it.".into()));
        }
        let selection = move_selection(
            request.files.as_ref(),
            state.facts.get("index").and_then(Json::as_str) == Some("live"),
        )?;
        let old = envs::runtime(conn, common.recipe.old)
            .await?
            .ok_or(UpdateError::NotFound)?;
        let kept_image = old
            .kept_image
            .as_ref()
            .map(|image| {
                serde_json::from_value::<orch_driver_contract::kept::KeptImage>(image.clone())
            })
            .transpose()
            .map_err(EngineError::from)?;
        let image_bytes = kept_image
            .as_ref()
            .map_or(0, |image| image.total_bytes.max(image.bytes));
        let mut template = hosts::template(conn, common.recipe.template)
            .await?
            .ok_or(UpdateError::NotFound)?;
        template.spec = common.recipe.definition.clone();
        template.driver.clone_from(&old.driver);
        let mut wanted =
            crate::places::wanted(conn, state.home_node_id, template, &state.config).await?;
        // A move may deliberately detach mounts absent on another host; its plan says which.
        wanted.mounts.clear();
        let places = crate::places::host_places_for_environment(
            conn,
            principal,
            &crate::places::Ask {
                home: state.home_node_id,
                wanted: &wanted,
                current: Some(old.host_id),
                minutes_idle: crate::places::IDLE_MINUTES_DEFAULT,
                now: self.inner.clock.now(),
            },
            environment,
        )
        .await?;
        let other: Vec<_> = places
            .into_iter()
            .filter(|p| p.host.id != old.host_id)
            .collect();
        let destination = crate::places::choose(&other, request.host_id)
            .map_err(|e| UpdateError::Blocked(vec![updates::blocker(e.code, e.message)]))?
            .clone();
        let mut blockers = common
            .blockers
            .into_iter()
            .filter(|b| b.code != "host_offline")
            .collect::<Vec<_>>();
        let target_machine =
            orch_db::queries::resources::machine(conn, destination.host.machine_id).await?;
        if target_machine
            .as_ref()
            .and_then(|machine| machine.facts.pointer("/policy/environment.remote_write"))
            .and_then(Json::as_bool)
            != Some(true)
        {
            blockers.push(updates::blocker("policy.remote_write_disabled","The destination machine policy does not allow remote writes; its managers can enable them."));
        }
        let borrowed = old
            .volumes
            .as_object()
            .into_iter()
            .flatten()
            .any(|(_, volume)| volume.get("directory").is_some_and(Json::is_object));
        if borrowed && request.copy_directories != Some(true) {
            blockers.push(updates::blocker("existing_directory_on_source_machine", "an existing workspace or home belongs to the source machine and cannot be moved; choose Copy existing directories into new storage made by Orch on the destination. The originals are kept."));
        }
        let now = self.inner.clock.now();
        let source_mounts = crate::mounts::views(conn, environment, now).await?;
        let workspace_storage = old
            .volumes
            .get("workspace")
            .and_then(|volume| volume.get("storage"))
            .and_then(Json::as_str)
            .map(str::to_owned);
        let plain = crate::mounts::resolve_for(
            conn,
            &crate::mounts::Target {
                environment,
                home: state.home_node_id,
                own: &state.config,
                owner: state.owner_id,
                machine: Some(destination.host.machine_id),
                driver: Some(&old.driver),
                workspace: &state.workspace_root,
                home_path: Some("/home/agent"),
                storage: workspace_storage.clone(),
                planned: &[],
                detached: &[],
            },
            now,
        )
        .await?
        .into_iter()
        .map(|mount| mount.view)
        .collect::<Vec<_>>();
        let source_host = hosts::host(conn, old.host_id).await?;
        let definition = orch_db::queries::jobs::changes::definition(conn, old.id).await?;
        let carry_limits = source_host
            .as_ref()
            .map(|host| {
                crate::isolation::explicit_subject_limits(
                    &host.drivers,
                    &old.driver,
                    definition.as_ref(),
                    orch_driver_contract::isolation::SUBJECT_VOLUME_CARRY,
                )
            })
            .unwrap_or_default();
        let decided = crate::mount_copies::decide(
            self,
            conn,
            &crate::mount_copies::Ask {
                choices: request.mounts.as_deref().unwrap_or_default(),
                check_fingerprints: request.check_fingerprints.unwrap_or(false).then(|| {
                    request
                        .fingerprint_request_id
                        .unwrap_or_else(orch_types::new_id)
                }),
                offer: crate::mount_copies::Offer::Everything,
                source: &source_mounts,
                target: &plain,
                source_machine: source_host.as_ref().map(|host| host.machine_id),
                target_machine: Some(destination.host.machine_id),
                principal,
                at: now,
            },
        )
        .await?;
        blockers.extend(decided.blockers);
        let target_mounts = if decided.decisions.detached.is_empty()
            && decided.decisions.copies.is_empty()
        {
            plain
        } else {
            let planned =
                decided
                    .decisions
                    .planned(Some(destination.host.machine_id), principal.user(), now);
            crate::mounts::resolve_for(
                conn,
                &crate::mounts::Target {
                    environment,
                    home: state.home_node_id,
                    own: &state.config,
                    owner: state.owner_id,
                    machine: Some(destination.host.machine_id),
                    driver: Some(&old.driver),
                    workspace: &state.workspace_root,
                    home_path: Some("/home/agent"),
                    storage: workspace_storage.clone(),
                    planned: &planned,
                    detached: &decided.decisions.detached,
                },
                now,
            )
            .await?
            .into_iter()
            .map(|mount| mount.view)
            .collect::<Vec<_>>()
        };
        let mounts = crate::mounts::plan(&source_mounts, &target_mounts, &decided.fated);
        let mut private_views = crate::mounts::private_plan(
            &source_mounts,
            &target_mounts,
            request.private_views.unwrap_or(true),
            &decided.decisions.detached,
        );
        let mcp = crate::mcp::plan(
            &crate::mcp::resolve(conn, environment, orch_types::Time::new(now)).await?,
            &crate::mcp::resolve_elsewhere(
                conn,
                &crate::mcp::Elsewhere::Moved {
                    environment,
                    machine: Some(destination.host.machine_id),
                },
                orch_types::Time::new(now),
            )
            .await?,
        );
        // The threads of the environment are read once, however many they are.
        let moved: std::collections::HashMap<Id, threads::ThreadRow> =
            threads::threads_among(conn, &common.recipe.threads)
                .await?
                .into_iter()
                .map(|row| (row.node_id, row))
                .collect();
        let local_agents = moved
            .values()
            .any(|row| row.arrangement.as_deref() != Some("separated"));
        let carry_home = request.home.unwrap_or(local_agents)
            || (request.copy_directories == Some(true)
                && old
                    .volumes
                    .get("home")
                    .and_then(|volume| volume.get("directory"))
                    .is_some_and(Json::is_object));
        let home = match crate::volume_carry::measure_for_move(
            self,
            conn,
            environment,
            carry_home,
            principal.user(),
            &mut private_views,
        )
        .await
        {
            Ok(home) => home,
            Err(_) => {
                if private_views.iter().any(|view| view.carried) {
                    blockers.push(updates::blocker("volume_measurement_unavailable", "the old host could not measure the whole home/private volumes; no carry can be confirmed"));
                }
                None
            }
        };
        let file_states = match crate::volume_carry::measure_file_states(
            self,
            conn,
            environment,
            principal.user(),
        )
        .await
        {
            Ok(measured) => measured,
            Err(_) => {
                blockers.push(updates::blocker("file_states_copy_unavailable", "the complete recorded-state store must be measured and carried before this move can start"));
                orch_api_contract::forks::FilesPlanFileStates {
                    measurement: None,
                    roots: 0,
                    objects: 0,
                    total_bytes: 0,
                    journals: 0,
                    complete: false,
                }
            }
        };
        if !file_states.complete && file_states.measurement.is_some() {
            blockers.push(updates::blocker(
                "measurement_pending",
                "Waiting for the recorded-state store count before verifying a complete move.",
            ));
        }
        if (home.is_some() || private_views.iter().any(|view| view.carried) || file_states.complete)
            && let Err(error) =
                crate::volume_carry::check_target(self, destination.host.machine_id, &old.driver)
                    .await
        {
            blockers.push(updates::blocker(
                "volume_copy_unavailable",
                error.to_string(),
            ));
        }
        // RuntimeLimit describes differences as well as outright refusals. Add
        // the relevant class explanation only when existing carry checks have
        // already blocked the move; do not turn an advisory into a new ban.
        if blockers.iter().any(|blocker| {
            matches!(
                blocker.code.as_str(),
                "volume_measurement_unavailable"
                    | "file_states_copy_unavailable"
                    | "volume_copy_unavailable"
                    | "measurement_pending"
            )
        }) {
            for limit in carry_limits {
                let kind = if limit.todo.is_some() {
                    "runtime_limit_text"
                } else {
                    "runtime_limit"
                };
                blockers.push(updates::blocker(
                    &format!("{kind}.{}", limit.key),
                    limit.sentence(),
                ));
            }
        }
        let (_, mut volume_bytes) = crate::volume_carry::totals(home.as_ref(), &private_views);
        if home.is_none() {
            volume_bytes = volume_bytes.saturating_add(file_states.total_bytes);
        }
        let source = match state.machine_id {
            Some(id) => resources::machine_place(conn, id).await?,
            None => None,
        };
        let live = source.as_ref().is_some_and(|s| s.online);
        let mut manifest = if live {
            let machine = state.machine_id.ok_or(UpdateError::NotFound)?;
            let command = Outgoing::new(
                new_id(),
                machine,
                SerialKey::Environment(environment),
                &FsManifest {
                    environment_id: environment,
                    selection: Selection {
                        include: selection.include.clone(),
                        exclude: selection.exclude.clone(),
                    },
                },
            )
            .map_err(EngineError::from)?;
            let id = command.id;
            let result = tokio::time::timeout(
                self.inner.config.hosting.attach_wait,
                crate::commands::run(self.inner.commands.as_ref(), command),
            )
            .await;
            match result {
                Ok(Ok(Outcome::Succeeded(value))) => {
                    serde_json::from_value::<FsManifestResult>(value)
                        .map_err(EngineError::from)?
                        .manifest_blob
                }
                Err(_) => {
                    self.inner
                        .commands
                        .cancel(id)
                        .await
                        .map_err(EngineError::from)?;
                    return Err(UpdateError::Blocked(vec![updates::blocker(
                        "source_unavailable",
                        "the source did not answer its workspace listing",
                    )]));
                }
                _ => {
                    return Err(UpdateError::Blocked(vec![updates::blocker(
                        "source_unavailable",
                        "the source could not list its workspace",
                    )]));
                }
            }
        } else {
            workspace_rows::environment(conn, environment)
                .await?
                .and_then(|r| r.manifest_blob)
                .ok_or_else(|| {
                    UpdateError::Blocked(vec![updates::blocker(
                        "files_unavailable",
                        "the source is offline and has no stored workspace listing",
                    )])
                })?
        };
        manifest =
            crate::selection::rebase_manifest(self, conn, &manifest, &state.workspace_root).await?;
        let mut listed = self.read_manifest(&manifest).await?;
        if !live && !selection.include.is_empty() {
            let matcher = crate::selection::Matcher::new(&selection)?;
            let selected = crate::manifest::build(
                listed.header(),
                listed
                    .entries()
                    .iter()
                    .filter(|entry| matcher.matches(&entry.path))
                    .cloned()
                    .collect(),
            )
            .map_err(EngineError::unreadable)?;
            manifest = self.put_manifest(conn, selected.bytes).await?.hash;
            listed = self.read_manifest(&manifest).await?;
        }
        let hashes = listed.hashes().collect::<Vec<_>>();
        let held = storage::blobs_among(conn, &hashes)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut file_count = 0;
        let mut total_bytes = 0_i64;
        let mut bytes_up = 0_i64;
        let mut missing = Vec::new();
        for entry in listed
            .entries()
            .iter()
            .filter(|e| e.kind == EntryKind::File)
        {
            file_count += 1;
            total_bytes = total_bytes.saturating_add(entry.size);
            let available = entry.hash.is_some_and(|h| held.contains(&h));
            if !available {
                bytes_up = bytes_up.saturating_add(entry.size);
                if !live {
                    missing.push(entry.path.clone());
                }
            }
        }
        if !missing.is_empty() {
            blockers.push(updates::blocker(
                "files_missing",
                "the cached workspace lacks file content; connect the source before moving it",
            ));
        }
        let mut actions = Vec::new();
        for id in &common.recipe.threads {
            let row = moved.get(id).ok_or(UpdateError::NotFound)?;
            let action = if row.arrangement.as_deref() == Some("separated") {
                if row
                    .executor
                    .as_deref()
                    .is_none_or(|kind| kind == "orch-exec")
                {
                    MoveThreadAction::Continue
                } else {
                    MoveThreadAction::Blocked
                }
            } else {
                MoveThreadAction::Reimport
            };
            if action == MoveThreadAction::Blocked {
                blockers.push(updates::blocker(
                    "executor_missing",
                    "The destination has no executor for an affected thread",
                ));
            }
            actions.push((*id, action));
        }
        let free = destination
            .place
            .storages
            .iter()
            .filter(|s| s.default)
            .map(|s| s.free_gb)
            .next()
            .unwrap_or(0);
        if free > 0
            && total_bytes
                .saturating_add(volume_bytes)
                .saturating_add(image_bytes)
                > free.saturating_mul(BYTES_PER_GIB)
        {
            blockers.push(updates::blocker(
                "no_capacity",
                "the destination storage cannot hold the selected workspace and owned volumes",
            ));
        }
        let source_descriptor = orch_db::queries::environment_facts::of(conn, environment).await?;
        let held_still = live
            && state.facts.get("suspend").and_then(Json::as_bool) == Some(true)
            && source_descriptor
                .as_ref()
                .and_then(|facts| facts.descriptor.as_ref())
                .and_then(|descriptor| descriptor.get("suspend"))
                .and_then(Json::as_bool)
                == Some(true);
        if kept_image.is_some() {
            let source_host = hosts::host(conn, old.host_id).await?;
            if source_host.as_ref().is_none_or(|host| !host.online) {
                blockers.push(updates::blocker(
                    "kept_image_source_offline",
                    "the old host must be online to carry its kept installed files",
                ));
            }
            for (place, descriptor) in [
                (
                    "source",
                    source_host
                        .as_ref()
                        .and_then(|host| host.drivers.get(&old.driver)),
                ),
                ("destination", destination.host.drivers.get(&old.driver)),
            ] {
                let support = descriptor
                    .and_then(|descriptor| descriptor.get("kept_file_transfer"))
                    .and_then(|support| {
                        serde_json::from_value::<
                                orch_driver_contract::descriptor::OperationSupport,
                            >(support.clone())
                            .ok()
                    })
                    .unwrap_or_default();
                if !support.supported {
                    blockers.push(updates::blocker("kept_image_transfer_unavailable", format!("{place}: {}", support.reason.unwrap_or_else(|| "this driver does not transfer kept installed files between hosts".into()))));
                }
            }
        }
        let files = FilesPlan {
            file_states: Some(file_states),
            kept_image_bytes: kept_image.as_ref().map(|image| image.bytes),
            source: if live {
                FilesSource::Live
            } else {
                FilesSource::Cached
            },
            source_as_of: listed.header().generated_at,
            file_count,
            total_bytes,
            bytes_to_upload: (if live { bytes_up } else { 0 })
                .saturating_add(volume_bytes)
                .saturating_add(image_bytes),
            bytes_to_download: total_bytes
                .saturating_add(volume_bytes)
                .saturating_add(image_bytes),
            missing: FilesMissing {
                count: missing.len() as i64,
                bytes: None,
                sample: missing,
            },
            path_map: PathMap {
                from: state.workspace_root.clone(),
                to: state.workspace_root.clone(),
            },
            home,
            private_views,
        };
        Ok(MoveDraft {
            mounts,
            mcp,
            recipe: MoveRecipe {
                copy_directories: borrowed && request.copy_directories == Some(true),
                kept_image,
                selection,
                replacement: common.recipe,
                destination: destination.host.id,
                named_host: request.host_id,
                source_machine: state.machine_id,
                source_status: state.status,
                manifest,
                held_still,
                files,
                mount_decisions: decided.decisions,
            },
            destination,
            threads: actions,
            blockers,
        })
    }
}

/// Queues a move with the same shared holds as a replacement.
pub async fn queue(
    conn: &mut Conn,
    draft: MoveDraft,
    batch: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<jobs::JobRow, UpdateError> {
    if !draft.blockers.is_empty() {
        return Err(UpdateError::Blocked(draft.blockers));
    }
    let recipe = draft.recipe;
    let row = crate::jobs::recreate::queue(
        conn,
        Draft {
            recipe: recipe.replacement.clone(),
            updates: Vec::new(),
            kept: Vec::new(),
            lost: Vec::new(),
            blockers: Vec::new(),
        },
        batch,
        now,
    )
    .await?;
    let plan = serde_json::to_value(&recipe).map_err(EngineError::from)?;
    let progress = serde_json::to_value(initial_progress(&PHASES)).map_err(EngineError::from)?;
    let row = jobs::changes::make_move(conn, row.id, &plan, &progress, now).await?;
    envs::set_status(
        conn,
        recipe.replacement.target.id(),
        &["recreating"],
        "moving",
        None,
        now,
    )
    .await?;
    Ok(row)
}

/// A workspace moved between managed hosts, with identity preserved at commit.
pub struct Moving {
    recipe: MoveRecipe,
}
impl Moving {
    /// Reads the move's durable inputs.
    pub fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            recipe: serde_json::from_value(row.plan.clone())?,
        })
    }
    fn environment(&self) -> Id {
        self.recipe.replacement.target.id()
    }

    async fn target_writable(&self, job: &Job) -> Result<(), Stop> {
        let mut conn = job.engine().db().acquire().await?;
        let host = hosts::host(&mut conn, self.recipe.destination)
            .await?
            .ok_or_else(|| fail("no_host", "The destination host is gone."))?;
        let machine = orch_db::queries::resources::machine(&mut conn, host.machine_id).await?;
        if machine
            .as_ref()
            .and_then(|machine| machine.facts.pointer("/policy/environment.remote_write"))
            .and_then(Json::as_bool)
            != Some(true)
        {
            return Err(fail(
                "policy.remote_write_disabled",
                "The destination machine policy does not allow remote writes.",
            ));
        }
        Ok(())
    }

    async fn lock(&self, job: &Job) -> Result<Done, Stop> {
        Recreating::of(&job.row().await?)?.run(job, "lock").await?;
        job.engine()
            .return_credentials_of(self.environment())
            .await
            .map_err(|e| fail("credential.return_failed", e.to_string()))?;
        if let Some(machine) = self.recipe.source_machine {
            let shells = {
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                changes::open_shells(&mut conn, self.environment()).await?
            };
            for terminal_id in shells {
                job.command(
                    Outgoing::new(
                        ids::derived("pty.close", &[job.id().as_bytes(), terminal_id.as_bytes()]),
                        machine,
                        SerialKey::Environment(self.environment()),
                        &PtyClose { terminal_id },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            }
            if self.recipe.held_still {
                job.remember(&json!({"suspend_requested":true})).await?;
                job.command(
                    Outgoing::new(
                        ids::derived("proc.suspend", &[job.id().as_bytes()]),
                        machine,
                        SerialKey::Environment(self.environment()),
                        &ProcSuspend {
                            environment_id: self.environment(),
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            }
        }
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        envs::close_shells(&mut conn, self.environment(), job.now()).await?;
        Ok(Done::Finished)
    }

    async fn provision(&self, job: &Job) -> Result<Done, Stop> {
        self.target_writable(job).await?;
        if let Some(image) = &self.recipe.kept_image {
            job.engine()
                .carry_kept_image(
                    job,
                    self.recipe.replacement.old,
                    self.recipe.destination,
                    image,
                )
                .await?;
        }
        if job.plan().await?.get("runtime").is_none() {
            let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
            let mut old = envs::runtime(&mut tx, self.recipe.replacement.old)
                .await?
                .ok_or_else(|| fail("runtime_gone", "the source runtime is gone"))?;
            let next = new_id();
            let text = next.simple().to_string();
            let suffix = &text[text.len() - NAME_DIGITS..];
            old.host_id = self.recipe.destination;
            old.machine_id = None;
            old.volumes = json!({"workspace":{"name":format!("orch-ws-{}-{suffix}",self.environment()),"storage":null},"home":{"name":format!("orch-home-{}-{suffix}",self.environment()),"storage":null},"state":{"name":format!("orch-state-{}-{suffix}",self.environment()),"storage":null}});
            let environment = envs::environment(&mut tx, self.environment())
                .await?
                .ok_or_else(|| fail("environment_gone", "the source environment is gone"))?;
            let host = hosts::host(&mut tx, self.recipe.destination)
                .await?
                .ok_or_else(|| fail("no_host", "the destination host is gone"))?;
            let planned = self.recipe.mount_decisions.planned(
                Some(host.machine_id),
                self.recipe.replacement.by,
                job.now(),
            );
            for mount in crate::mounts::resolve_for(
                &mut tx,
                &crate::mounts::Target {
                    environment: self.environment(),
                    home: environment.home_node_id,
                    own: &environment.config,
                    owner: environment.owner_id,
                    machine: Some(host.machine_id),
                    driver: Some(&old.driver),
                    workspace: &environment.workspace_root,
                    home_path: Some("/home/agent"),
                    storage: None,
                    planned: &planned,
                    detached: &self.recipe.mount_decisions.detached,
                },
                job.now(),
            )
            .await?
            {
                if mount.layer.is_some() {
                    old.volumes[format!("over:{}", mount.view.mount.id)] = json!({"name":format!("orch-over-{}-{}-{suffix}", self.environment(), mount.view.mount.id),"storage":null});
                }
            }
            changes::successor(
                &mut tx,
                &old,
                next,
                &format!("orch-environment-{suffix}"),
                old.template_id,
                &old.spec_hash,
                job.now(),
            )
            .await?;
            sqlx::query("UPDATE runtimes SET kept_image=$2 WHERE id=$1")
                .bind(next)
                .bind(self.recipe.kept_image.as_ref().map(|image| json!(image)))
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"runtime":next,"host":self.recipe.destination,"fresh":true,"mode":"make", "volume_carry":crate::volume_carry::Carry {source_runtime:self.recipe.replacement.old, selectors:crate::volume_carry::carried(Some(&self.recipe.files))}}),
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
        }
        let row = job.row().await?;
        for phase in ["image", "volumes", "tools", "create", "enroll", "attach"] {
            Provision::of(job.engine().clone(), &row)
                .run(job, phase)
                .await?;
        }
        Ok(Done::Finished)
    }

    async fn copy(&self, job: &Job) -> Result<Done, Stop> {
        self.target_writable(job).await?;
        let mut snapshot = if self.recipe.files.source == FilesSource::Live {
            let machine = self
                .recipe
                .source_machine
                .ok_or_else(|| fail("source_unavailable", "the source connector is gone"))?;
            let value = job
                .command_part(
                    Outgoing::new(
                        ids::derived("fs.snapshot", &[job.id().as_bytes()]),
                        machine,
                        SerialKey::Environment(self.environment()),
                        &FsSnapshot {
                            work: None,
                            environment_id: self.environment(),
                            job_id: job.id(),
                            selection: self.recipe.selection.clone(),
                        },
                    )
                    .map_err(EngineError::from)?,
                    0.0..0.5,
                )
                .await?;
            serde_json::from_value::<FsSnapshotResult>(value).map_err(EngineError::from)?
        } else {
            FsSnapshotResult {
                manifest_blob: self.recipe.manifest,
                file_count: self.recipe.files.file_count,
                total_bytes: self.recipe.files.total_bytes,
                uploaded_bytes: 0,
                missing: self.recipe.files.missing.count,
            }
        };
        if snapshot.missing > 0 {
            return Err(fail(
                "files_missing",
                "the source could not provide all of the promised files",
            ));
        }
        {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            snapshot.manifest_blob = crate::selection::rebase_manifest(
                job.engine(),
                &mut conn,
                &snapshot.manifest_blob,
                &self.recipe.files.path_map.from,
            )
            .await?;
        }
        let manifest = job.engine().read_manifest(&snapshot.manifest_blob).await?;
        let mut pins = manifest.hashes().collect::<Vec<_>>();
        pins.push(snapshot.manifest_blob);
        pins.sort();
        pins.dedup();
        let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
        if jobs::lock(&mut tx, job.id())
            .await?
            .is_some_and(|r| r.plan.get("move_pins").is_none())
        {
            storage::pin_blobs(&mut tx, &pins).await?;
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"move_pins":pins,"move_snapshot":snapshot}),
                job.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        let plan = job.plan().await?;
        let next: Id =
            serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let (machine, _) = hosts::machine_of_runtime(&mut conn, next)
            .await?
            .ok_or_else(|| fail("unhealthy", "the destination has no connector"))?;
        drop(conn);
        let root = plan
            .pointer("/candidate_facts/workspace_root")
            .and_then(Json::as_str)
            .ok_or_else(|| fail("unhealthy", "the destination has no workspace"))?;
        let value = job
            .command_part(
                Outgoing::new(
                    ids::derived("fs.materialize", &[job.id().as_bytes()]),
                    machine,
                    SerialKey::Environment(self.environment()),
                    &FsMaterialize {
                        work: None,
                        environment_id: self.environment(),
                        job_id: Some(job.id()),
                        manifest_blob: snapshot.manifest_blob,
                        target_root: root.to_owned(),
                        purpose: MaterializePurpose::Move,
                    },
                )
                .map_err(EngineError::from)?,
                if self.recipe.files.source == FilesSource::Live {
                    0.5..1.0
                } else {
                    0.0..1.0
                },
            )
            .await?;
        let copied: FsMaterializeResult =
            serde_json::from_value(value).map_err(EngineError::from)?;
        if copied.files + copied.skipped < snapshot.file_count {
            return Err(fail(
                "files_missing",
                "the destination did not report every promised file",
            ));
        }
        job.command(
            Outgoing::new(
                ids::derived("move.worktrees.repair", &[job.id().as_bytes()]),
                machine,
                SerialKey::Environment(self.environment()),
                &orch_link_contract::source::GitWorktreeRepair {
                    environment_id: self.environment(),
                    job_id: job.id(),
                    manifest_blob: snapshot.manifest_blob,
                    previous_root: self.recipe.files.path_map.from.clone(),
                    target_root: root.to_owned(),
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
        job.remember(&json!({"copied":copied,"target_root":root}))
            .await?;
        Ok(Done::Finished)
    }

    async fn verify(&self, job: &Job) -> Result<Done, Stop> {
        self.target_writable(job).await?;
        let plan = job.plan().await?;
        if plan.get("copied").is_none() {
            return Err(fail("files_missing", "the workspace was not materialized"));
        }
        if let Some(expected) = &self.recipe.files.file_states {
            let runtime: Id =
                serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
            crate::volume_carry::verify_file_states(job, runtime, expected).await?;
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            let (machine, _) = hosts::machine_of_runtime(&mut conn, runtime)
                .await?
                .ok_or_else(|| {
                    fail("unhealthy", "the recorded-state candidate has no connector")
                })?;
            drop(conn);
            let value = job
                .command(
                    Outgoing::new(
                        ids::derived("move.file-states.content.verify", &[job.id().as_bytes()]),
                        machine,
                        SerialKey::Environment(self.environment()),
                        &orch_link_contract::source::FsRecordVerify {
                            environment_id: self.environment(),
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            let verified: orch_link_contract::source::FsRecordVerifyResult =
                serde_json::from_value(value).map_err(EngineError::from)?;
            if verified.roots < expected.roots {
                return Err(fail(
                    "file_states_copy_unavailable",
                    "the candidate did not verify every planned recorded root",
                ));
            }
            job.remember(&json!({ "file_states_content_verified": true }))
                .await?;
        } else {
            return Err(fail(
                "file_states_copy_unavailable",
                "make a new move plan that carries its recorded-state store",
            ));
        }
        Recreating::of(&job.row().await?)?.run(job, "verify").await
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        self.target_writable(job).await?;
        let plan = job.plan().await?;
        let next: Id =
            serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
        let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
        let row = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| fail("job_gone", "the move's job is gone"))?;
        if row.cancel_requested() {
            return Err(Stop::Cancelled);
        }
        let old = envs::lock_environment(&mut tx, self.environment())
            .await?
            .ok_or_else(|| fail("environment_gone", "the environment is gone"))?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let principal = Principal::load(
            &mut tx,
            orch_authz::Caller::session(self.recipe.replacement.by),
            orch_types::Time::new(job.now()),
        )
        .await
        .map_err(EngineError::from)?;
        let mut template = hosts::template(&mut tx, self.recipe.replacement.template)
            .await?
            .ok_or_else(|| fail("template_unavailable", "The destination template is gone."))?;
        template.spec = self.recipe.replacement.definition.clone();
        let runtime = envs::runtime(&mut tx, next)
            .await?
            .ok_or_else(|| fail("runtime_gone", "The destination unit is gone."))?;
        template.driver.clone_from(&runtime.driver);
        let mut wanted =
            crate::places::wanted(&mut tx, old.home_node_id, template, &old.config).await?;
        wanted.mounts.clear();
        wanted.limits = self.recipe.replacement.resolved_resources.clone();
        wanted.needs = crate::places::needs_of(
            &wanted.template.spec,
            old.config
                .get(orch_settings_contract::registry::keys::ENV_NEEDS),
            Some(&wanted.limits),
        );
        let placements = crate::places::host_places_for_environment(
            &mut tx,
            &principal,
            &crate::places::Ask {
                home: old.home_node_id,
                wanted: &wanted,
                current: None,
                minutes_idle: crate::places::IDLE_MINUTES_DEFAULT,
                now: job.now(),
            },
            old.id,
        )
        .await?;
        let admission = placements
            .into_iter()
            .find(|place| {
                place.host.id == self.recipe.destination
                    && (place.usable()
                        || place.place.reasons.iter().all(|reason| {
                            matches!(
                                reason,
                                orch_api_contract::environment::HostReason::Full
                                    | orch_api_contract::environment::HostReason::NoRoom
                            )
                        }))
            })
            .and_then(|place| place.created)
            .ok_or_else(|| {
                fail(
                    "access.denied",
                    "The destination no longer admits this environment through Host Use.",
                )
            })?;
        if !orch_authz::environments::reserve_capacity(&mut tx, &admission)
            .await
            .map_err(EngineError::from)?
        {
            return Err(fail(
                "no_capacity",
                "The destination Use entry has no remaining environment capacity.",
            ));
        }
        let from_row =
            orch_db::queries::environments::thread_engine::environment(&mut tx, self.environment())
                .await?
                .ok_or_else(|| fail("environment_gone", "the environment is gone"))?;
        let from: orch_api_contract::environment::EnvironmentRef = serde_json::from_value(
            crate::views::environment_ref_json(
                &mut tx,
                &from_row,
                orch_types::Time::new(job.now()),
            )
            .await?,
        )
        .map_err(EngineError::from)?;
        // The copies' locations become the destination's before the candidate does, so that only the environments
        // that had the mount there before are told their location changed.
        crate::mount_copies::commit(
            &mut tx,
            &self.recipe.mount_decisions,
            self.environment(),
            job.id(),
            self.recipe.replacement.by,
            job.now(),
        )
        .await?;
        if !changes::commit(
            &mut tx,
            self.recipe.replacement.old,
            next,
            plan.get("candidate_facts"),
            job.now(),
        )
        .await?
        {
            if row.plan["committed"] == true {
                return Ok(Done::Finished);
            }
            return Err(fail(
                "conflict",
                "the candidate is no longer this move's successor",
            ));
        }
        envs::set_entries(
            &mut tx,
            old.id,
            admission.host.as_ref().and_then(|(entry, _)| *entry),
            admission.template.as_ref().and_then(|(entry, _)| *entry),
        )
        .await?;
        changes::moved_settings(
            &mut tx,
            self.environment(),
            self.recipe.named_host,
            self.recipe.replacement.old,
            self.recipe.replacement.options.keep_previous,
            job.now(),
        )
        .await?;
        if !self.recipe.mount_decisions.detached.is_empty() {
            // What the plan detached stays detached wherever the environment's unit is made again.
            let mut config = envs::environment(&mut tx, self.environment())
                .await?
                .ok_or_else(|| fail("environment_gone", "the environment is gone"))?
                .config;
            crate::mount_copies::detach_in(&mut config, &self.recipe.mount_decisions.detached);
            sqlx::query(
                "UPDATE environments SET config=$2, revision=revision+1, updated_at=$3 WHERE id=$1",
            )
            .bind(self.environment())
            .bind(&config)
            .bind(job.now())
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        }
        if self.recipe.copy_directories {
            sqlx::query("UPDATE environments SET config=jsonb_set(config,'{env.data_sources}',$2), revision=revision+1, updated_at=$3 WHERE id=$1")
                .bind(self.environment()).bind(json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}}))
                .bind(job.now()).execute(&mut *tx).await.map_err(DbError::from)?;
        }
        let after = envs::environment(&mut tx, self.environment())
            .await?
            .ok_or_else(|| fail("environment_gone", "the environment is gone"))?;
        let to_row =
            orch_db::queries::environments::thread_engine::environment(&mut tx, self.environment())
                .await?
                .ok_or_else(|| fail("environment_gone", "the environment is gone"))?;
        let to: orch_api_contract::environment::EnvironmentRef = serde_json::from_value(
            crate::views::environment_ref_json(&mut tx, &to_row, orch_types::Time::new(job.now()))
                .await?,
        )
        .map_err(EngineError::from)?;
        let by = serde_json::from_value(
            crate::views::user_ref_json(&mut tx, self.recipe.replacement.by).await?,
        )
        .map_err(EngineError::from)?;
        let path_map = PathMap {
            from: old.workspace_root,
            to: after.workspace_root.clone(),
        };
        let marker = orch_api_contract::threads::MarkerPayload::EnvironmentMoved(
            orch_api_contract::threads::MarkerEnvironmentMoved {
                from,
                to,
                path_map,
                mounts_detached: Vec::new(),
                by,
            },
        );
        let mut unit = Unit::new(&mut tx, job.now());
        for id in &self.recipe.replacement.threads {
            let Some(thread) = threads::lock(&mut *unit.conn, *id).await? else {
                continue;
            };
            let seq = unit.stamp(*id).await?;
            if let Some(segment) = thread.current_segment_id {
                let _ = seq;

                crate::ingest::add_marker(
                    &mut unit,
                    *id,
                    segment,
                    None,
                    &serde_json::to_value(&marker).map_err(EngineError::from)?,
                    &format!("moved:{}", job.id()),
                )
                .await?;
            }
        }
        let finished = unit.finish().await?;
        crate::recovery::commit_threads(
            job,
            &mut tx,
            self.environment(),
            after.generation,
            self.recipe.replacement.by,
            self.recipe.files.home.is_some(),
            false,
        )
        .await?;
        let mut audit = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(self.recipe.replacement.by),
            orch_db::audit::name!("environment.moved"),
            "environment",
            Some(self.environment()),
            "The environment moved to another managed host",
        );
        audit.environment_id = Some(self.environment());
        audit.node_id = Some(after.home_node_id);
        audit.history = true;
        audit.details = json!({"job":job.id(),"old_runtime":self.recipe.replacement.old,"runtime":next,"host":self.recipe.destination});
        crate::resource_facts::library::created(
            &mut tx,
            self.recipe.replacement.template,
            Some(self.recipe.destination),
            self.environment(),
            false,
            Some(self.recipe.replacement.by),
            orch_types::Time::new(job.now()),
        )
        .await?;
        orch_db::audit::append(
            &mut tx,
            &audit,
            &orch_db::audit::Origin::default(),
            job.now(),
        )
        .await?;
        jobs::merge_plan(&mut tx, job.id(), &json!({"committed":true}), job.now()).await?;
        hosts::delete_runtime_secret(&mut tx, next, "enroll_token").await?;
        job.engine().commit(tx, finished).await?;
        Ok(Done::Finished)
    }

    async fn cleanup(&self, job: &Job) -> Result<Done, Stop> {
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let old = envs::runtime(&mut conn, self.recipe.replacement.old)
            .await?
            .ok_or_else(|| fail("runtime_gone", "the old runtime is gone"))?;
        let host = hosts::host(&mut conn, old.host_id)
            .await?
            .ok_or_else(|| fail("no_host", "the source host is gone"))?;
        drop(conn);
        match job
            .command(
                Outgoing::new(
                    ids::derived("node.runtime.stop", &[job.id().as_bytes(), b"cleanup"]),
                    host.machine_id,
                    SerialKey::Runtime(old.id),
                    &NodeRuntimeStop {
                        runtime_id: old.id,
                        reason: Some(orch_driver_contract::runtime::UnitReason::Move),
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await
        {
            Ok(_) => Ok(Done::Finished),
            Err(Stop::Failed(_)) => Ok(Done::Skipped(
                "the old unit is retired; its reaper will finish stopping it".to_owned(),
            )),
            Err(e) => Err(e),
        }
    }

    async fn rollback(&self, job: &Job) -> Result<Option<String>, Stop> {
        let plan = job.plan().await?;
        if plan["committed"] == true {
            return Ok(None);
        }
        let mut cleanup_detail = None;
        if let Some(next) = plan
            .get("runtime")
            .cloned()
            .and_then(|v| serde_json::from_value::<Id>(v).ok())
        {
            let cleanup: Result<(), Stop> = async {
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                let runtime = envs::runtime(&mut conn, next)
                    .await?
                    .ok_or_else(|| fail("runtime_gone", "the candidate is gone"))?;
                let host = hosts::host(&mut conn, runtime.host_id)
                    .await?
                    .ok_or_else(|| fail("no_host", "the destination host is gone"))?;
                drop(conn);
                job.command_to_undo(
                    Outgoing::new(
                        ids::derived("node.runtime.remove", &[job.id().as_bytes(), b"undo"]),
                        host.machine_id,
                        SerialKey::Runtime(next),
                        &NodeRuntimeRemove {
                            runtime_id: next,
                            keep_volumes: true,
                            volume_refs: Vec::new(),
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
                for (role, volume) in runtime.volumes.as_object().into_iter().flatten() {
                    if volume.get("directory").is_some_and(Json::is_object) {
                        continue;
                    }
                    if let Some(name) = volume["name"].as_str() {
                        job.command_to_undo(
                            Outgoing::new(
                                ids::derived(
                                    "node.volume.remove",
                                    &[job.id().as_bytes(), role.as_bytes()],
                                ),
                                host.machine_id,
                                SerialKey::Volume(name.to_owned()),
                                &NodeVolumeRemove {
                                    driver: runtime.driver.clone(),
                                    name: name.to_owned(),
                                    storage: volume["storage"].as_str().map(str::to_owned),
                                    labels: BTreeMap::new(),
                                },
                            )
                            .map_err(EngineError::from)?,
                        )
                        .await?;
                    }
                }
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                hosts::revoke_machine_of(&mut conn, next, job.now()).await?;
                changes::discard(&mut conn, next, job.now()).await?;
                hosts::delete_runtime_secret(&mut conn, next, "enroll_token").await?;
                drop(conn);
                // With the candidate gone nothing uses the copies' volumes; a replaced location gets its data back.
                let left = crate::mount_copies::undo(job).await;
                if !left.is_empty() {
                    return Err(fail("mount_copy_failed", left.join("; ")));
                }
                Ok(())
            }
            .await;
            if let Err(error) = cleanup {
                // Source processes must be resumed even when the destination will only answer
                // a later cleanup attempt. Keep the candidate for the reaper, never publish it.
                let detail = format!("candidate cleanup is pending after rollback: {error:?}");
                let mut conn = job
                    .engine()
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                envs::retire_runtime(&mut conn, next, Some(&detail), job.now()).await?;
                hosts::revoke_machine_of(&mut conn, next, job.now()).await?;
                drop(conn);
                job.remember(&json!({"cleanup_pending":next,"cleanup_detail":detail}))
                    .await?;
                cleanup_detail = Some(detail);
            }
        }
        let mut guard_conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let guarded = crate::network::blocked(&mut guard_conn, self.recipe.replacement.old)
            .await?
            .is_some();
        drop(guard_conn);
        if !guarded
            && plan["suspend_requested"] == true
            && let Some(machine) = self.recipe.source_machine
        {
            job.command_to_undo(
                Outgoing::new(
                    ids::derived("proc.resume", &[job.id().as_bytes(), b"undo"]),
                    machine,
                    SerialKey::Environment(self.environment()),
                    &ProcResume {
                        environment_id: self.environment(),
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        }
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        envs::set_status(
            &mut conn,
            self.environment(),
            &["moving"],
            if guarded {
                "stopped"
            } else {
                &self.recipe.source_status
            },
            None,
            job.now(),
        )
        .await?;
        Ok(cleanup_detail)
    }
}

#[async_trait]
impl Phased for Moving {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "lock" => self.lock(job).await,
            "provision" => self.provision(job).await,
            "copy" => self.copy(job).await,
            "history" => {
                let plan = job.plan().await?;
                let next: Id =
                    serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
                let mut conn = job.engine().db().acquire().await?;
                let (machine, _) = hosts::machine_of_runtime(&mut conn, next)
                    .await?
                    .ok_or_else(|| fail("unhealthy", "The destination has no connector"))?;
                let generation = envs::runtime(&mut conn, next)
                    .await?
                    .ok_or_else(|| fail("runtime_gone", "The destination unit is gone"))?
                    .generation;
                drop(conn);
                crate::recovery::prepare_histories(
                    job,
                    self.environment(),
                    machine,
                    plan["target_root"].as_str().ok_or_else(|| {
                        fail("files_missing", "The target workspace is not known")
                    })?,
                    generation,
                    self.recipe.files.home.is_some(),
                )
                .await
            }
            "after_up" => super::after_up::run(job).await,
            "verify" => self.verify(job).await,
            "commit" => self.commit(job).await,
            "cleanup" => self.cleanup(job).await,
            _ => Err(fail("internal", format!("a move has no phase {phase}"))),
        }
    }
    async fn undo(&self, job: &Job, _: &Stop) -> Undone {
        if super::after_up::record_interrupted(job).await.is_err() {
            tracing::warn!("an interrupted move command outcome could not be recorded");
        }
        match self.rollback(job).await {
            Ok(None) => Undone::Everything,
            Ok(Some(detail)) => Undone::Partly(detail),
            Err(e) => Undone::Partly(format!("the move could not be fully undone: {e:?}")),
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        Some(
            json!({"environment":self.environment(),"host":self.recipe.destination,"runtime":job.plan().await.ok()?.get("runtime")}),
        )
    }
}

impl Engine {
    /// Releases the move's transient file pins and thread holds after its runner closes it.
    pub(crate) async fn after_move(&self, row: &jobs::JobRow) {
        if row.plan.get("portable").is_some() {
            self.after_machine_move(row).await;
            return;
        }
        if let Err(error) = self.cleanup_moved_image(row).await {
            tracing::warn!(job=%row.id,%error,"move image cleanup waits for the node sweep");
        }
        if let Ok(pins) = serde_json::from_value::<Vec<Hash>>(row.plan["kept_image_pins"].clone())
            && let Ok(mut conn) = self.db().acquire().await
        {
            let _ = storage::unpin_blobs(&mut conn, &pins, self.inner.clock.now()).await;
        }
        if let Ok(pins) = serde_json::from_value::<Vec<Hash>>(row.plan["move_pins"].clone())
            && let Ok(mut conn) = self.db().acquire().await
            && let Err(error) = storage::unpin_blobs(&mut conn, &pins, self.inner.clock.now()).await
        {
            tracing::warn!(%error,"the move's transient pins could not be released");
        }
        self.after_recreate(row).await;
    }
}
