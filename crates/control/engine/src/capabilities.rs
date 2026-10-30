//! Requests decided on the host's side by scope, revocation and recreation consequences.
//!
//! A capability widens what an environment may do to the machine it runs on: a debugger, a virtualisation device,
//! a network privilege. What an environment has and can be given is worked out here from four things: the
//! environment (what it was granted), the template it was made from (what it has from the start, and what may ever
//! be granted), the entry its creator used on that template (which of those it may be given), and the catalogue
//! of definitions (a capability is offered only where its definition has a patch for the template's driver). A
//! connected environment, a directory on a person's machine, has none of this.
//!
//! **A request** (`request_capability`, in [`crate::tools::request_capability`]) is decided by the policy of the
//! capability as it holds at the environment's home (`capability.policy`, else the definition's own default): it
//! is given, refused, or asked of whoever decides on the machine's side. That is the people of the scope the
//! policy names, and never the people whose work asked: the holders of `host.manage` on the environment's host
//! (`host`), the admins of its home org (`org`), or the system's admins (`system`). Owning the thread, the topic or
//! the environment decides nothing here. Gateway requests use holders of `ports.manage` on the environment.
//! A request nobody decides lapses after the resolved `requests.expiry_hours`.
//!
//! **A grant** adds the key to what the environment was given, writes `capability.granted`, puts a line in the
//! requesting thread's history, and asks for the environment's unit to be made anew with the capability's patch
//! applied. Making it anew belongs to the part that runs the units of environments: here a job of that kind is
//! queued with the reason, and nothing more is done to the unit. The same holds for the grant's failure, which
//! takes the key out again (the job's part), and for the notices around a recreate.
//!
//! **A denial** writes `capability.denied` and queues, for the requesting thread, the system input that says who
//! denied it and why ("an approver", where the thread's place has `collab.attribution = never`, and the note kept). **A revocation** by whoever manages the environment or its host takes the key out, writes
//! `capability.revoked`, puts a line in the history of the threads of the environment, and queues the same job.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::unit::Unit;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef as AccessObject, Refusal};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::approvals::{ApprovalDecision, CapabilityDecision};
use orch_authz::{Act, Caller, Principal, holders, nodes, resources};
use orch_db::audit::{Actor, Entry, Origin, append};
use orch_db::queries::approvals::{self as approval_rows, ApprovalRow, capability as requests};
use orch_db::queries::environments::capabilities::{
    self as env_rows, DefinitionRow, EnvironmentFacts, RecreateJob,
};
use orch_db::queries::threads;
use orch_settings::{Place as SettingsPlace, Snapshot, Wanted};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::BTreeMap;

pub use crate::approvals::ApprovalError;

/// The text said to a model when its request is granted at once, and its placeholders. A place may replace it
/// under the key `capability_granted` of `agent.messages`.
const GRANTED: &str = "\"<label>\" was granted. <effect>";
/// The text said to a model when its request waits for a decision (`capability_pending`).
const PENDING: &str = "Requested \"<label>\". If approved: <effect> You will receive the decision. Continue with work that does not need it.";
/// The text said to a model that asks for what the environment does not offer (`capability_not_offered`).
const NOT_OFFERED: &str = "\"<capability>\" is not offered by this environment. Offered: <keys>.";
/// The system input for a request that was denied (`capability_denied`).
const DENIED: &str = "Your request for \"<label>\" was denied by <display name><note>.";
/// Who denied a request, where the asking thread's place has `collab.attribution = never`: the texts Orch composes
/// name nobody there.
const AN_APPROVER: &str = "an approver";
/// The system input for a request nobody decided in time (`capability_expired`).
const EXPIRED: &str = "Your request for \"<label>\" expired without a decision.";
/// Said when the environment is no place for capabilities.
pub const DO_NOT_APPLY: &str = "Capabilities do not apply to this environment.";

/// A successful live grant changes authority immediately without stopping any work.
pub const GRANTED_LIVE_TEXT: &str =
    "\"<label>\" is available now. Nothing stops; existing processes and all files are kept.";
/// A policy request distinguishes approval from application and states the possible failure recovery.
pub const PENDING_LIVE_TEXT: &str = "Requested \"<label>\". If approved, its live application is attempted without stopping work. It is available only after application succeeds; a failed attempt may restart the environment to restore its previous permissions. You will receive the outcome.";

/// How many requests the sweep lets lapse at once: a handful more than there ever are in a minute.
const LAPSE_BATCH: i64 = 50;
/// A capability by its key and what it is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The key a request names.
    pub key: String,
    /// What it is called.
    pub label: String,
}

/// What an environment has and what it can be given.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentView {
    /// What was found of the environment.
    pub facts: EnvironmentFacts,
    /// Whether capabilities apply to it at all.
    pub applies: bool,
    /// What it holds: from its template and by grant, by key.
    pub held: Vec<Listed>,
    /// What may be asked for: allowed, offered by its driver, and not held.
    pub offered: Vec<Listed>,
    /// What it runs on and under, and the limits there that touch what it has or can be given.
    pub isolation: Option<orch_driver_contract::isolation::RuntimeIsolation>,
}

fn strings(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_owned)
        .collect()
}

/// What `capability.policy` says of a key at a place, else the definition's own default: whether a request is
/// allowed, asked about or denied, and whose approvers decide it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// `allow`, `ask` or `deny`.
    pub decision: String,
    /// `host`, `org` or `system`.
    pub approver_scope: String,
}

impl Engine {
    /// The same consequences appear on the approval card and in the tool answer.
    pub(crate) async fn capability_effect(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
        key: &str,
    ) -> Result<String, EngineError> {
        if self.capability_is_live(conn, environment, key).await? {
            return Ok("This capability becomes available without stopping work when its live application succeeds. If application fails, the grant is not made and the environment may restart to restore its previous permissions; the failure is reported with the decision.".into());
        }
        let Some(runtime) =
            orch_db::queries::environments::hosting::live_runtime(conn, environment).await?
        else {
            return Ok("This capability is applied when the environment's first unit is made; there is no unit to stop.".into());
        };
        let host = orch_db::queries::resources::hosting::host(conn, runtime.host_id).await?;
        // What the driver declares for the runtime this unit was made under, not for the driver as a whole.
        let definition = orch_db::queries::jobs::changes::definition(conn, runtime.id).await?;
        let driver = host.as_ref().and_then(|host| {
            crate::isolation::unit_driver(&host.drivers, &runtime.driver, definition.as_ref())
        });
        let restart = driver
            .as_ref()
            .and_then(|driver| driver.get("recreate"))
            .and_then(Json::as_str)
            == Some("restart");
        let preserves = restart
            || driver
                .as_ref()
                .and_then(|driver| driver.get("recreate_preserves_files"))
                .and_then(Json::as_bool)
                == Some(true);
        let grace = self.recreate_grace_secs(conn, environment).await?;
        let mut effect = format!(
            "The environment will be recreated after up to {grace} seconds waiting for running work. Running processes stop; {}",
            if preserves {
                "everything on disk is kept, including installed programs in /usr, /etc and /opt."
                    .to_owned()
            } else {
                format!(
                    "persistent volumes are kept. {}",
                    crate::isolation::REPLACED_PLAN_NOTE
                )
            }
        );
        if key == "docker_inside" && crate::isolation::image_lacks(definition.as_ref(), "docker") {
            effect.push(' ');
            effect.push_str(crate::isolation::DOCKER_MISSING_NOTE);
        }
        if preserves && !restart {
            let estimate = match host {
                Some(host) if host.online => {
                    self.estimate_runtime_files(host.machine_id, runtime.id)
                        .await?
                }
                _ => None,
            };
            effect.push(' ');
            effect.push_str(&crate::kept_images::recording_note(estimate));
        }
        Ok(effect)
    }
    /// Freezes this driver's patches and keeps template defaults distinct from revocable environment grants.
    pub(crate) async fn prepared_capabilities(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
        driver: &str,
        definition: &Json,
    ) -> Result<orch_driver_contract::capabilities::CapabilityState, EngineError> {
        let facts = env_rows::environment_facts(conn, environment)
            .await?
            .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        let mut allowed = strings(definition.get("allowed_capabilities"));
        if let Some(entry) = facts
            .entry_options
            .as_ref()
            .and_then(|options| options.get("capabilities"))
        {
            let entry = strings(Some(entry));
            allowed.retain(|key| entry.contains(key));
        }
        let base = strings(definition.get("base_capabilities"));
        let definitions = env_rows::definitions(conn).await?;
        let patches = definitions
            .into_iter()
            .filter(|capability| {
                allowed.contains(&capability.key) || base.contains(&capability.key)
            })
            .filter_map(|capability| {
                capability
                    .driver_patch
                    .get(driver)
                    .filter(|patch| !patch.is_null())
                    .cloned()
                    .map(|patch| (capability.key, patch))
            })
            .collect();
        let granted = facts
            .granted_capabilities
            .into_iter()
            .filter(|key| allowed.contains(key) || base.contains(key))
            .collect();
        Ok(orch_driver_contract::capabilities::CapabilityState {
            allowed,
            base,
            granted,
            patches,
        })
    }

    /// Whether the current owned unit, rather than a later template, declares this capability live.
    pub(crate) async fn capability_is_live(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
        key: &str,
    ) -> Result<bool, EngineError> {
        if orch_driver_contract::capabilities::gateway_given(key) {
            return Ok(true);
        }
        let Some(runtime) =
            orch_db::queries::environments::hosting::live_runtime(conn, environment).await?
        else {
            return Ok(false);
        };
        if orch_db::queries::jobs::changes::definition(conn, runtime.id)
            .await?
            .and_then(|spec| spec.get("capabilities").cloned())
            .is_none()
        {
            return Ok(false);
        }
        let Some((_, drivers)) = env_rows::host_of(conn, environment).await? else {
            return Ok(false);
        };
        let definition = orch_db::queries::jobs::changes::definition(conn, runtime.id).await?;
        Ok(
            crate::isolation::unit_driver(&drivers, &runtime.driver, definition.as_ref())
                .as_ref()
                .and_then(|driver| driver.pointer("/live_changes/capabilities"))
                .and_then(Json::as_array)
                .is_some_and(|keys| keys.iter().any(|given| given.as_str() == Some(key))),
        )
    }
    /// What an environment has and can be given, for the answer to a request and for the instructions.
    pub async fn environment_capabilities(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
    ) -> Result<Option<EnvironmentView>, EngineError> {
        let Some(facts) = env_rows::environment_facts(conn, environment).await? else {
            return Ok(None);
        };
        if facts.deleted_at.is_some() {
            return Ok(None);
        }
        let spec = facts.template_spec.clone().unwrap_or(Json::Null);
        let definitions = env_rows::definitions(conn).await?;
        let driver = facts.template_driver.clone();
        let machine = orch_db::queries::environment_facts::of(conn, environment).await?;
        let described = machine
            .as_ref()
            .and_then(|machine| machine.descriptor.as_ref());
        let mut applies = described
            .and_then(|d| d.get("capabilities"))
            .and_then(Json::as_object)
            .is_some_and(|declared| !declared.is_empty());
        let label_of = |key: &str| {
            definitions
                .iter()
                .find(|def| def.key == key)
                .map_or_else(|| key.to_owned(), |def| def.label.clone())
        };
        let native_applies = applies;
        let mut allowed = strings(spec.get("allowed_capabilities"));
        // Gateway boundaries are declared independently from native driver patches; a template narrows them.
        if spec.get("allowed_capabilities").is_none() {
            allowed.extend(
                strings(described.and_then(|d| d.pointer("/live_changes/gateway_given")))
                    .into_iter()
                    .filter(|key| definitions.iter().any(|definition| definition.key == *key)),
            );
        }
        applies |= allowed
            .iter()
            .any(|key| orch_driver_contract::capabilities::gateway_given(key));
        let mut held_keys = strings(spec.get("base_capabilities"));
        held_keys.extend(facts.granted_capabilities.iter().cloned());
        held_keys.sort_unstable();
        held_keys.dedup();
        let entry_allows = facts
            .entry_options
            .as_ref()
            .and_then(|options| options.get("capabilities"))
            .map(|listed| strings(Some(listed)));
        let mut offered: Vec<Listed> = Vec::new();
        if applies {
            for key in allowed {
                let gateway = orch_driver_contract::capabilities::gateway_given(&key)
                    && definitions.iter().any(|def| def.key == key);
                let patched = gateway
                    || (native_applies
                        // The driver's declaration for the runtime this environment is under names the key.
                        && described
                            .and_then(|d| d.get("capabilities"))
                            .and_then(Json::as_object)
                            .is_some_and(|declared| declared.contains_key(&key))
                        && definitions.iter().any(|def| {
                            def.key == key
                                && driver.as_ref().is_some_and(|driver| {
                                    def.driver_patch
                                        .get(driver)
                                        .is_some_and(|patch| !patch.is_null())
                                })
                        }));
                let entry_ok = entry_allows
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&key));
                if patched && entry_ok && !held_keys.contains(&key) {
                    offered.push(Listed {
                        label: label_of(&key),
                        key,
                    });
                }
            }
        }
        offered.sort_by(|a, b| a.key.cmp(&b.key));
        offered.dedup_by(|a, b| a.key == b.key);
        let held: Vec<Listed> = held_keys
            .iter()
            .map(|key| Listed {
                key: key.clone(),
                label: label_of(key),
            })
            .collect();
        // What it runs on and under: the limits that touch what it holds, what it may be given and what its template
        // allows, and the mounts it has.
        let has_mounts = orch_settings::get::<Json>(
            conn,
            SettingsPlace::Environment(environment),
            keys::ENV_MOUNTS,
        )
        .await?
        .and_then(|mounts| {
            mounts
                .as_object()
                .map(|mounts| mounts.values().any(|mode| mode != &Json::Bool(false)))
        })
        .unwrap_or(false);
        let template_keys = strings(spec.get("allowed_capabilities"));
        let isolation = described.map(|described| {
            let subjects = crate::isolation::subjects(
                held_keys
                    .iter()
                    .map(String::as_str)
                    .chain(offered.iter().map(|listed| listed.key.as_str()))
                    .chain(template_keys.iter().map(String::as_str)),
                has_mounts,
            );
            crate::isolation::isolation_from_descriptor(
                driver.as_deref().unwrap_or_default(),
                described,
                &subjects,
            )
        });
        Ok(Some(EnvironmentView {
            facts,
            applies,
            held,
            offered,
            isolation,
        }))
    }

    /// What the policy says of a capability at the home of an environment.
    pub async fn capability_policy(
        &self,
        conn: &mut orch_db::Conn,
        home: Id,
        definition: &DefinitionRow,
    ) -> Result<Policy, EngineError> {
        let wanted = [crate::tools::definition_of(keys::CAPABILITY_POLICY)?];
        let snapshot =
            Snapshot::load(conn, SettingsPlace::Node(home), Wanted::Keys(&wanted)).await?;
        let table: BTreeMap<String, Json> =
            crate::tools::setting(&snapshot, keys::CAPABILITY_POLICY)?.unwrap_or_default();
        let word = |entry: Option<&Json>, member: &str, fallback: &str| {
            entry
                .and_then(|entry| entry.get(member))
                .and_then(Json::as_str)
                .unwrap_or(fallback)
                .to_owned()
        };
        let entry = table.get(&definition.key);
        Ok(Policy {
            decision: word(entry, "decision", &definition.default_decision),
            approver_scope: word(entry, "approver_scope", &definition.default_approver_scope),
        })
    }

    /// The seconds the recreate a grant causes waits for running turns, as the environment's settings say.
    pub async fn recreate_grace_secs(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
    ) -> Result<i64, EngineError> {
        let wanted = [crate::tools::definition_of(
            keys::CAPABILITY_RECREATE_GRACE_SECS,
        )?];
        let snapshot = Snapshot::load(
            conn,
            SettingsPlace::Environment(environment),
            Wanted::Keys(&wanted),
        )
        .await?;
        Ok(crate::tools::setting(&snapshot, keys::CAPABILITY_RECREATE_GRACE_SECS)?.unwrap_or(0))
    }

    /// Gives an environment a capability, in the transaction of `unit`: the key is added, the grant audited, a line
    /// put in the history of the requesting thread, and the recreation asked for. The answer says whether this call
    /// gave it.
    pub(crate) async fn grant_capability(
        &self,
        unit: &mut Unit<'_>,
        facts: &EnvironmentFacts,
        definition: &DefinitionRow,
        requester: Option<Id>,
        decided_by: Option<Id>,
    ) -> Result<bool, EngineError> {
        if facts.granted_capabilities.contains(&definition.key) {
            return Ok(false);
        }
        if self
            .queue_live_capability(unit, facts, &definition.key, true, requester, decided_by)
            .await?
            .is_some()
        {
            return Ok(true);
        }
        let granted = self
            .record_capability_grant(unit, facts, definition, requester, decided_by)
            .await?;
        if granted && !orch_driver_contract::capabilities::gateway_given(&definition.key) {
            self.queue_recreation(unit, facts.id, &definition.key, decided_by, "capability")
                .await?;
        }
        Ok(granted)
    }

    pub(crate) async fn queue_live_capability(
        &self,
        unit: &mut Unit<'_>,
        facts: &EnvironmentFacts,
        key: &str,
        grant: bool,
        requester: Option<Id>,
        decided_by: Option<Id>,
    ) -> Result<Option<RecreateJob>, EngineError> {
        if orch_driver_contract::capabilities::gateway_given(key)
            || !self
                .capability_is_live(&mut *unit.conn, facts.id, key)
                .await?
        {
            return Ok(None);
        }
        let Some(runtime) =
            orch_db::queries::environments::hosting::live_runtime(&mut *unit.conn, facts.id)
                .await?
        else {
            return Ok(None);
        };
        let Some(recorded) =
            orch_db::queries::jobs::changes::definition(&mut *unit.conn, runtime.id).await?
        else {
            return Ok(None);
        };
        let Some(capabilities) = recorded.get("capabilities") else {
            return Ok(None);
        };
        let mut before: orch_driver_contract::capabilities::CapabilityState =
            serde_json::from_value(capabilities.clone())?;
        before.granted = facts.granted_capabilities.clone();
        let mut after = before.clone();
        if grant {
            if !after.granted.iter().any(|held| held == key) {
                after.granted.push(key.into());
            }
        } else {
            after.granted.retain(|held| held != key);
        }
        let now = unit.now();
        let row = crate::jobs::capability_change::queue(
            &mut *unit.conn,
            facts.id,
            &crate::jobs::capability_change::Plan {
                runtime: runtime.id,
                capability: key.into(),
                grant,
                before,
                after,
                requester,
                decided_by,
            },
            now,
        )
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?;
        Ok(Some(RecreateJob {
            id: row.id,
            environment_id: facts.id,
            state: row.state,
            plan: row.plan,
            created_by: row.created_by,
            created_at: row.created_at,
        }))
    }

    /// Records a grant only after a live node acknowledgment, or immediately when its next unit must apply it.
    pub(crate) async fn record_capability_grant(
        &self,
        unit: &mut Unit<'_>,
        facts: &EnvironmentFacts,
        definition: &DefinitionRow,
        requester: Option<Id>,
        decided_by: Option<Id>,
    ) -> Result<bool, EngineError> {
        let now = unit.now();
        if !env_rows::grant(&mut *unit.conn, facts.id, &definition.key, now).await? {
            return Ok(false);
        }
        let mut entry = Entry::new(
            decided_by.map_or(Actor::System, Actor::User),
            orch_db::audit::action!(AuditAction::CapabilityGranted).as_str(),
            "environment",
            Some(facts.id),
            "A capability was given to an environment.",
        );
        entry.history = AuditAction::CapabilityGranted.is_history_line();
        entry.environment_id = Some(facts.id);
        entry.org_id = Some(facts.org_id);
        entry.details = json!({ "capability": definition.key, "decided_by": decided_by });
        append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
        if let Some(thread) = requester {
            self.put_capability_marker(
                unit,
                thread,
                "capability_granted",
                &definition.key,
                decided_by,
            )
            .await?;
        }
        Ok(true)
    }

    /// The live application still queued for this exact key, separately from a pending approval card.
    pub(crate) async fn pending_live_capability(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
        key: &str,
    ) -> Result<Option<Id>, EngineError> {
        Ok(
            orch_db::queries::jobs::open_of_environment(conn, environment)
                .await?
                .into_iter()
                .find(|job| {
                    job.kind == crate::jobs::capability_change::KIND
                        && job.plan.get("capability").and_then(Json::as_str) == Some(key)
                })
                .map(|job| job.id),
        )
    }

    /// Writes a line of history in a thread: a capability was given or taken back.
    async fn put_capability_marker(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        marker: &str,
        capability: &str,
        by: Option<Id>,
    ) -> Result<(), EngineError> {
        let Some(row) = threads::thread(&mut *unit.conn, thread).await? else {
            return Ok(());
        };
        let Some(segment) = row.current_segment_id else {
            return Ok(());
        };
        let by = match by {
            Some(person) => crate::views::user_ref_json(&mut *unit.conn, person).await?,
            None => Json::Null,
        };
        let payload = json!({ "marker": marker, "data": { "capability": capability, "by": by } });
        let key = format!("{marker}:{capability}:{}", unit.now().timestamp_millis());
        crate::ingest::add_marker(unit, thread, segment, None, &payload, &key).await
    }

    /// Asks for the unit of an environment to be made anew, for the reason given. The jobs of environments are
    /// run by the part that runs their units; this queues the job, which that part takes up, and leaves the unit as
    /// it is.
    async fn queue_recreation(
        &self,
        unit: &mut Unit<'_>,
        environment: Id,
        capability: &str,
        by: Option<Id>,
        reason: &str,
    ) -> Result<Option<RecreateJob>, EngineError> {
        let now = unit.now();
        if orch_db::queries::environments::hosting::live_runtime(&mut *unit.conn, environment)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let state =
            orch_db::queries::environments::hosting::environment(&mut *unit.conn, environment)
                .await?
                .ok_or_else(|| EngineError::unreadable("the capability's environment is gone"))?;
        // Authority has already been decided by the capability's policy and scoped approvers.
        // Inspect the existing definition without requiring an unrelated EnvironmentManage grant.
        let principal = Principal::load(
            &mut *unit.conn,
            Caller::session(by.unwrap_or(state.owner_id)),
            Time::new(now),
        )
        .await?;
        let mut draft = self
            .inspect_update(
                &mut *unit.conn,
                &principal,
                crate::updates::Target::Environment(environment),
                orch_api_contract::environment::UpdatePreflightReason::Recreate,
                None,
            )
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        draft.recipe.capability = Some(capability.into());
        draft.recipe.capability_reason = Some(reason.into());
        let grace = self
            .recreate_grace_secs(&mut *unit.conn, environment)
            .await?;
        draft.recipe.capability_grace_secs = Some(grace);
        draft.recipe.options.wait_minutes = (grace + 59) / 60;
        let row = crate::jobs::recreate::queue(&mut *unit.conn, draft, None, now)
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        Ok(Some(RecreateJob {
            id: row.id,
            environment_id: environment,
            state: row.state,
            plan: row.plan,
            created_by: row.created_by,
            created_at: row.created_at,
        }))
    }

    // ------------------------------------------------------------ deciding

    /// The owned runtime's host, or the explicitly selected physical host before
    /// the first runtime exists. An unplaced environment names no host manager.
    pub(crate) async fn capability_host(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
    ) -> Result<Option<(Id, Json)>, EngineError> {
        if let Some(host) = env_rows::host_of(conn, environment).await? {
            return Ok(Some(host));
        }
        let Some(facts) = orch_db::queries::environment_facts::of(conn, environment).await? else {
            return Ok(None);
        };
        if !facts.makes_units() {
            return Ok(None);
        }
        let Some(machine) = facts.machine_id else {
            return Ok(None);
        };
        Ok(
            sqlx::query_as("SELECT id,drivers FROM hosts WHERE machine_id=$1")
                .bind(machine)
                .fetch_optional(conn)
                .await
                .map_err(orch_db::DbError::from)?,
        )
    }

    /// The people who decide a request of this scope for this environment, nearest first: the holders of
    /// `host.manage` on its host, the admins of its home org, or the system's admins.
    pub async fn capability_approvers(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
        scope: &str,
    ) -> Result<Vec<Id>, EngineError> {
        let now = Time::new(self.inner.clock.now());
        let Some(facts) = env_rows::environment_facts(conn, environment).await? else {
            return Ok(Vec::new());
        };
        let (object, permission) = match scope {
            "environment" => (
                AccessObject {
                    kind: ObjectKind::Environment,
                    id: environment,
                },
                Permission::PortsManage,
            ),
            "host" => match self.capability_host(conn, environment).await? {
                Some((host, _)) => (
                    AccessObject {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                    Permission::HostManage,
                ),
                None => return Ok(Vec::new()),
            },
            "org" => (
                AccessObject {
                    kind: ObjectKind::Org,
                    id: home_org(conn, facts.home_node_id)
                        .await?
                        .unwrap_or(facts.org_id),
                },
                Permission::AccessManage,
            ),
            "system" => return Ok(holders::notification_admins(conn, now).await?),
            _ => return Ok(Vec::new()),
        };
        Ok(orch_authz::approvals::permission_deciders(conn, object, permission, now).await?)
    }

    /// Whether a person is among those who decide a request.
    async fn may_decide_capability(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        approval: &ApprovalRow,
    ) -> Result<Result<(), Box<Refusal>>, EngineError> {
        let Some(environment) = approval.environment_id else {
            return Ok(Err(Box::new(Refusal::of(
                Permission::HostManage,
                AccessObject {
                    kind: ObjectKind::Environment,
                    id: approval.id,
                },
                Vec::new(),
            ))));
        };
        let scope = approval
            .approver_scope
            .clone()
            .unwrap_or_else(|| "system".to_owned());
        let now = Time::new(unit.now());
        let principal = Principal::load(&mut *unit.conn, caller.clone(), now).await?;
        if scope == "environment" {
            return match resources::check(
                &mut *unit.conn,
                &principal,
                AccessObject {
                    kind: ObjectKind::Environment,
                    id: environment,
                },
                Permission::PortsManage,
                orch_authz::Policy::default(),
            )
            .await?
            {
                Decision::Allowed { .. } => Ok(Ok(())),
                Decision::Refused(refusal) => Ok(Err(Box::new(refusal))),
            };
        }
        if principal.is_system_admin() {
            return Ok(Ok(()));
        }
        let allowed = match scope.as_str() {
            "environment" => resources::check(
                &mut *unit.conn,
                &principal,
                AccessObject {
                    kind: ObjectKind::Environment,
                    id: environment,
                },
                Permission::PortsManage,
                orch_authz::Policy::default(),
            )
            .await?
            .is_allowed(),
            "host" => match self.capability_host(&mut *unit.conn, environment).await? {
                Some((host, _)) => matches!(
                    resources::check(
                        &mut *unit.conn,
                        &principal,
                        AccessObject {
                            kind: ObjectKind::Host,
                            id: host,
                        },
                        Permission::HostManage,
                        orch_authz::Policy::default(),
                    )
                    .await?,
                    Decision::Allowed { .. }
                ),
                None => false,
            },
            "org" => {
                let facts = env_rows::environment_facts(&mut *unit.conn, environment).await?;
                match facts {
                    Some(facts) => {
                        let org = home_org(&mut *unit.conn, facts.home_node_id)
                            .await?
                            .unwrap_or(facts.org_id);
                        matches!(
                            nodes::check(
                                &mut *unit.conn,
                                &principal,
                                org,
                                Permission::AccessManage,
                                Act::Use,
                                orch_authz::Policy::default(),
                            )
                            .await?,
                            Decision::Allowed { .. }
                        )
                    }
                    None => false,
                }
            }
            _ => false,
        };
        if allowed {
            return Ok(Ok(()));
        }
        let (permission, kind) = match scope.as_str() {
            "environment" => (Permission::PortsManage, ObjectKind::Environment),
            "host" => (Permission::HostManage, ObjectKind::Host),
            _ => (Permission::AccessManage, ObjectKind::Org),
        };
        Ok(Err(Box::new(Refusal::of(
            permission,
            AccessObject {
                kind,
                id: environment,
            },
            Vec::new(),
        ))))
    }

    /// Whether a person is among those who decide a request for a capability, as the scope its policy named says.
    pub async fn capability_decision_authority(
        &self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        approval: &ApprovalRow,
    ) -> Result<Result<(), Box<Refusal>>, EngineError> {
        let mut unit = Unit::new(conn, principal.now().get());
        self.may_decide_capability(&mut unit, principal.caller(), approval)
            .await
    }

    /// Whether a person is among those who decide a request for a capability, as the scope its policy named says.
    pub async fn may_decide_capability_request(
        &self,
        caller: &Caller,
        approval: &ApprovalRow,
    ) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let mut unit = Unit::new(&mut conn, self.inner.clock.now());
        Ok(self
            .may_decide_capability(&mut unit, caller, approval)
            .await?
            .is_ok())
    }

    /// Decides a request for a capability for a person who is among those who decide it. The first decision to
    /// reach the database wins. Approving gives the capability (and asks for the recreation); denying tells the
    /// requesting thread by a system input.
    pub async fn decide_capability(
        &self,
        caller: &Caller,
        approval_id: Id,
        decision: &ApprovalDecision,
    ) -> Result<ApprovalRow, ApprovalError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now))
            .await
            .map_err(EngineError::from)?;
        self.decide_capability_guarded(tx, principal, approval_id, decision)
            .await
    }

    /// Decides as [`Self::decide_capability`] does, in a transaction whose caller was asked again once the
    /// request had come (the API's action guard): `principal` was loaded in `tx` by that check, and its caller
    /// and its time are the ones that decide.
    pub async fn decide_capability_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        approval_id: Id,
        decision: &ApprovalDecision,
    ) -> Result<ApprovalRow, ApprovalError> {
        let ApprovalDecision::Capability(CapabilityDecision { approve, note }) = decision else {
            return Err(ApprovalError::Invalid(
                "a request for a capability is answered with approve and, if you like, a note"
                    .to_owned(),
            ));
        };
        let caller = principal.caller().clone();
        let caller = &caller;
        let now = principal.now().get();
        let (finished, row, deciders) = {
            let mut unit = Unit::new(&mut tx, now);
            let approval = approval_rows::approval(&mut *unit.conn, approval_id)
                .await?
                .filter(|row| row.kind == "capability")
                .ok_or(ApprovalError::NotFound)?;
            if approval.state != "pending" {
                return Err(ApprovalError::AlreadyDecided);
            }
            if let Err(refusal) = self
                .may_decide_capability(&mut unit, caller, &approval)
                .await?
            {
                return Err(ApprovalError::Denied(refusal));
            }
            let state = if *approve { "approved" } else { "denied" };
            let stored = serde_json::to_value(decision).map_err(EngineError::from)?;
            if !approval_rows::decide(
                &mut *unit.conn,
                approval_id,
                state,
                Some(caller.user()),
                &stored,
                now,
            )
            .await?
            {
                return Err(ApprovalError::AlreadyDecided);
            }
            self.settle_capability(
                &mut unit,
                &approval,
                *approve,
                Some(caller.user()),
                note.as_deref(),
            )
            .await?;
            let row = approval_rows::approval(&mut *unit.conn, approval_id)
                .await?
                .ok_or(ApprovalError::NotFound)?;
            (unit.finish().await?, row, Vec::<Id>::new())
        };
        self.commit(tx, finished).await?;
        self.inner.live.approvals_changed(&deciders);
        Ok(row)
    }

    /// What follows a decision of a request, in the transaction that made it.
    async fn settle_capability(
        &self,
        unit: &mut Unit<'_>,
        approval: &ApprovalRow,
        approved: bool,
        by: Option<Id>,
        note: Option<&str>,
    ) -> Result<(), EngineError> {
        let now = unit.now();
        let key = approval
            .request
            .get("capability")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned();
        let label = approval
            .request
            .get("label")
            .and_then(Json::as_str)
            .unwrap_or(&key)
            .to_owned();
        let Some(environment) = approval.environment_id else {
            return Ok(());
        };
        let messages = match approval.thread_id {
            Some(thread) => {
                crate::tools::ToolSettings::load(&mut *unit.conn, thread)
                    .await?
                    .messages
            }
            None => BTreeMap::new(),
        };
        if approved {
            let facts = env_rows::environment_facts(&mut *unit.conn, environment).await?;
            let definition = env_rows::definition(&mut *unit.conn, &key).await?;
            if let (Some(facts), Some(definition)) = (facts, definition) {
                self.grant_capability(unit, &facts, &definition, approval.thread_id, by)
                    .await?;
                if let Some(job) = self
                    .pending_live_capability(&mut *unit.conn, environment, &key)
                    .await?
                {
                    orch_db::queries::jobs::merge_plan(
                        &mut *unit.conn,
                        job,
                        &json!({"approval":approval.id}),
                        now,
                    )
                    .await?;
                    sqlx::query("UPDATE approvals SET decision=COALESCE(decision,'{}'::jsonb) || jsonb_build_object('application',$2::jsonb) WHERE id=$1")
                        .bind(approval.id).bind(json!({"job_id":job,"state":"applying"}))
                        .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                }
            }
            return Ok(());
        }
        let mut entry = Entry::new(
            by.map_or(Actor::System, Actor::User),
            orch_db::audit::action!(AuditAction::CapabilityDenied).as_str(),
            "environment",
            Some(environment),
            "A request for a capability was denied.",
        );
        entry.environment_id = Some(environment);
        entry.details = json!({ "capability": key, "approval": approval.id, "note": note });
        append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
        // The person who denied it, as the place shows people ("Name (@handle)"); "an approver" where the asking
        // thread's place names nobody in what Orch composes.
        let names_people = match approval.thread_id {
            Some(thread) => crate::resolved::attribution(&mut *unit.conn, thread)
                .await?
                .names_people(),
            None => true,
        };
        let who = match by {
            Some(_) if !names_people => AN_APPROVER.to_owned(),
            Some(person) => threads::people(&mut *unit.conn, &[person])
                .await?
                .into_iter()
                .next()
                .map(|person| {
                    orch_chat::written::person_name(&person.display_name, &person.username)
                })
                .unwrap_or_default(),
            None => "policy".to_owned(),
        };
        let note_text = note
            .filter(|note| !note.trim().is_empty())
            .map(|note| format!(": {}", note.trim()))
            .unwrap_or_default();
        let text = messages
            .get("capability_denied")
            .map_or(DENIED, String::as_str)
            .replace("<label>", &label)
            .replace("<display name>", &who)
            .replace("<note>", &note_text)
            .replace("<capability>", &key);
        Self::system_input(unit, approval.thread_id, &text).await
    }

    /// Queues, for a thread, an input that no person sent: it is asked of the thread's first owner when its turn is
    /// about to start.
    pub(crate) async fn system_input(
        unit: &mut Unit<'_>,
        thread: Option<Id>,
        text: &str,
    ) -> Result<(), EngineError> {
        let Some(thread_id) = thread else {
            return Ok(());
        };
        let Some(_row) = threads::lock(&mut *unit.conn, thread_id).await? else {
            return Ok(());
        };
        let id = ids::derived(
            "capability.outcome",
            &[thread_id.as_bytes(), text.as_bytes()],
        )
        .to_string();
        let notice = json!({"id":id,"kind":"capability_outcome","text":format!("<orch>{}</orch>",crate::instructions::escape_body(text)),"at":unit.now()});
        if threads::notices::append(&mut *unit.conn, thread_id, &notice).await? {
            let seq = unit.stamp(thread_id).await?;
            threads::set_seq_state(&mut *unit.conn, thread_id, seq).await?;
        }
        Ok(())
    }

    /// Lets the requests nobody decided in time lapse, and tells the threads that asked. Called by the supervisor.
    pub(crate) async fn expire_capability_requests(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let due = {
            let mut conn = self.inner.db.acquire().await?;
            requests::overdue(&mut conn, now, LAPSE_BATCH).await?
        };
        for approval in due {
            let mut tx = self.inner.db.begin().await?;
            let finished = {
                let mut unit = Unit::new(&mut tx, now);
                if requests::expire(&mut *unit.conn, approval.id, now).await? {
                    let label = approval
                        .request
                        .get("label")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let messages = match approval.thread_id {
                        Some(thread) => {
                            crate::tools::ToolSettings::load(&mut *unit.conn, thread)
                                .await?
                                .messages
                        }
                        None => BTreeMap::new(),
                    };
                    let text = messages
                        .get("capability_expired")
                        .map_or(EXPIRED, String::as_str)
                        .replace("<label>", &label);
                    Self::system_input(&mut unit, approval.thread_id, &text).await?;
                }
                unit.finish().await?
            };
            self.commit(tx, finished).await?;
        }
        Ok(())
    }

    /// Takes a capability back from an environment, for a person who manages the environment or its host: the key is
    /// removed, `capability.revoked` written, the threads that work there told in their history, and the
    /// environment's unit asked to be made anew without it.
    pub async fn revoke_capability(
        &self,
        caller: &Caller,
        environment: Id,
        key: &str,
    ) -> Result<Option<RecreateJob>, RevokeError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let (finished, job) = {
            let mut unit = Unit::new(&mut tx, now);
            let facts = env_rows::environment_facts(&mut *unit.conn, environment)
                .await?
                .filter(|facts| facts.deleted_at.is_none())
                .ok_or(RevokeError::NotFound)?;
            let principal =
                Principal::load(&mut *unit.conn, caller.clone(), Time::new(now)).await?;
            let object = AccessObject {
                kind: ObjectKind::Environment,
                id: environment,
            };
            let manages = matches!(
                resources::check(
                    &mut *unit.conn,
                    &principal,
                    object,
                    if orch_driver_contract::capabilities::gateway_given(key) {
                        Permission::PortsManage
                    } else {
                        Permission::EnvironmentManage
                    },
                    orch_authz::Policy::default(),
                )
                .await?,
                Decision::Allowed { .. }
            );
            let hosts = if orch_driver_contract::capabilities::gateway_given(key) {
                false
            } else {
                match env_rows::host_of(&mut *unit.conn, environment).await? {
                    Some((host, _)) => matches!(
                        resources::check(
                            &mut *unit.conn,
                            &principal,
                            AccessObject {
                                kind: ObjectKind::Host,
                                id: host,
                            },
                            Permission::HostManage,
                            orch_authz::Policy::default(),
                        )
                        .await?,
                        Decision::Allowed { .. }
                    ),
                    None => false,
                }
            };
            if !manages && !hosts {
                return Err(RevokeError::Denied(Box::new(Refusal::of(
                    if orch_driver_contract::capabilities::gateway_given(key) {
                        Permission::PortsManage
                    } else {
                        Permission::EnvironmentManage
                    },
                    object,
                    Vec::new(),
                ))));
            }
            if !env_rows::revoke(&mut *unit.conn, environment, key, now).await? {
                return Err(RevokeError::NotGranted);
            }
            let label = env_rows::definition(&mut *unit.conn, key)
                .await?
                .map_or_else(|| key.to_owned(), |def| def.label);
            let mut entry = Entry::new(
                Actor::User(caller.user()),
                orch_db::audit::action!(AuditAction::CapabilityRevoked).as_str(),
                "environment",
                Some(environment),
                "A capability was taken back from an environment.",
            );
            entry.history = AuditAction::CapabilityRevoked.is_history_line();
            entry.environment_id = Some(environment);
            entry.org_id = Some(facts.org_id);
            entry.details = json!({ "capability": key });
            append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
            for thread in orch_db::queries::threads::tools::threads_in_environment(
                &mut *unit.conn,
                environment,
            )
            .await?
            {
                self.put_capability_marker(
                    &mut unit,
                    thread,
                    "capability_revoked",
                    key,
                    Some(caller.user()),
                )
                .await?;
            }
            let reason = format!("\"{label}\" was removed");
            let job = if orch_driver_contract::capabilities::gateway_given(key) {
                None
            } else if let Some(job) = self
                .queue_live_capability(&mut unit, &facts, key, false, None, Some(caller.user()))
                .await?
            {
                Some(job)
            } else {
                self.queue_recreation(&mut unit, environment, key, Some(caller.user()), &reason)
                    .await?
            };
            (unit.finish().await?, job)
        };
        self.commit(tx, finished).await?;
        Ok(job)
    }
}

/// Why a capability was not taken back.
#[derive(Debug, thiserror::Error)]
pub enum RevokeError {
    /// There is no such environment.
    #[error("there is no such environment")]
    NotFound,
    /// The environment was not given that capability.
    #[error("the environment was not given that capability")]
    NotGranted,
    /// The caller manages neither the environment nor its host.
    #[error("the caller manages neither the environment nor its host")]
    Denied(Box<Refusal>),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for RevokeError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for RevokeError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// The nearest org on the path of a node, which is the home of what the node is the home of.
async fn home_org(conn: &mut orch_db::Conn, node: Id) -> Result<Option<Id>, EngineError> {
    Ok(orch_db::queries::threads::tools::nearest_org(conn, node).await?)
}

/// What a model is told, with a place's own words where it gave them.
#[must_use]
pub fn said(
    messages: &BTreeMap<String, String>,
    key: &str,
    shipped: &str,
    replacements: &[(&str, &str)],
) -> String {
    let mut text = messages.get(key).map_or(shipped, String::as_str).to_owned();
    for (placeholder, value) in replacements {
        text = text.replace(placeholder, value);
    }
    text
}

/// The shipped words of what a model is told when its request is granted at once.
pub const GRANTED_TEXT: &str = GRANTED;
/// The shipped words of what a model is told when its request waits.
pub const PENDING_TEXT: &str = PENDING;
/// The shipped words of what a model is told when the key is not offered.
pub const NOT_OFFERED_TEXT: &str = NOT_OFFERED;
