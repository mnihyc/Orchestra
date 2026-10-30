//! Asynchronous environment capability requests.
//!
//! `request_capability` asks for something the agent's environment lacks, by the key the agent's instructions list.
//! The call returns at once, in every case, with one of three answers: `granted` (the capability is the
//! environment's now, or already was), `pending` (a person who decides on the machine's side was asked) or `denied`
//! (this environment is none to ask, the key is not offered there, or policy says no), with a sentence that tells
//! the agent what happens next. A name the environment does not know is answered with the keys it does, so the tool
//! also answers what can be asked for.
//!
//! What a request comes to is the table of [`crate::capabilities`]: the environment must be a unit made from a
//! template, the key must be among what that template, the entry its creator used on it and the catalogue allow,
//! a key already held is available, one already asked for is pending under the same request, and the policy of
//! the capability, as it holds where the environment lives, allows, asks or denies. Asking raises an approval of
//! the kind `capability` that the people of the policy's scope decide; it lapses after
//! `capability.approval_expiry_hours`.
//!
//! The same call asked again finds what it made: the request has an identifier derived from the call.

use crate::capabilities::{DO_NOT_APPLY, GRANTED_TEXT, NOT_OFFERED_TEXT, PENDING_TEXT, said};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::tools::definitions::{REASON_MAX_CHARS, Refusal};
use crate::tools::{Answer, CallContext, ToolSettings};
use crate::unit::Unit;
use chrono::Duration;
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{Actor, Entry, Origin, append};
use orch_db::queries::approvals::{self as approval_rows, capability as requests};
use orch_db::queries::environments::capabilities as env_rows;
use orch_db::queries::threads::ThreadRow;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde::Deserialize;
use serde_json::json;

/// The arguments of `request_capability`, once the schema has let them through.
#[derive(Debug, Deserialize)]
struct Arguments {
    capability: String,
    reason: String,
}

/// A repeated identical denied boundary request waits one hour before raising another card.
const DENIAL_HOLD_HOURS: i64 = 1;
/// The shared requests registry defaults to one day when a snapshot has no explicit value.
const REQUEST_EXPIRY_DEFAULT_HOURS: i64 = 24;

/// What is said when the key is none of the environment's.
fn none_offered() -> &'static str {
    "none"
}

fn done(status: &str, notice: &str, request: Option<Id>) -> Answer {
    let mut result = json!({ "status": status, "notice": notice });
    if let Some(request) = request {
        result["request_id"] = json!(request);
    }
    Answer::Done(result)
}

impl Engine {
    /// `request_capability`: the call of an agent.
    pub(crate) async fn request_capability_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
        settings: &ToolSettings,
    ) -> Result<Answer, EngineError> {
        let arguments: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(arguments) => arguments,
            Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
        };
        if arguments.reason.trim().is_empty() || arguments.reason.chars().count() > REASON_MAX_CHARS
        {
            return Ok(
                Refusal::invalid("\"reason\" is one sentence of 1 to 1000 characters.").into(),
            );
        }
        let key = arguments.capability.trim().to_owned();
        let messages = &settings.messages;
        let mut conn = self.inner.db.acquire().await?;
        let Some(environment) = thread.environment_id else {
            return Ok(done("denied", DO_NOT_APPLY, None));
        };
        let Some(view) = self
            .environment_capabilities(&mut conn, environment)
            .await?
        else {
            return Ok(done("denied", DO_NOT_APPLY, None));
        };
        if !view.applies {
            return Ok(done("denied", DO_NOT_APPLY, None));
        }
        let label_of = |listed: &crate::capabilities::Listed| listed.label.clone();
        // What the environment holds is available, whether or not it could be given again.
        if let Some(held) = view.held.iter().find(|held| held.key == key) {
            return Ok(done(
                "granted",
                &format!("\"{}\" is already available.", label_of(held)),
                None,
            ));
        }
        let Some(offer) = view.offered.iter().find(|offered| offered.key == key) else {
            let keys = if view.offered.is_empty() {
                none_offered().to_owned()
            } else {
                view.offered
                    .iter()
                    .map(|offered| offered.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let mut notice = said(
                messages,
                "capability_not_offered",
                NOT_OFFERED_TEXT,
                &[("<capability>", &key), ("<keys>", &keys)],
            );
            // Where the runtime the environment is under cannot give it, say why.
            if let Some(why) = view
                .isolation
                .as_ref()
                .and_then(|isolation| isolation.unavailable.get(&key))
            {
                notice.push_str(&format!(" It cannot be given under this runtime: {why}."));
            }
            return Ok(done("denied", &notice, None));
        };
        let Some(definition) = env_rows::definition(&mut conn, &key).await? else {
            return Ok(done("denied", DO_NOT_APPLY, None));
        };
        let label = offer.label.clone();
        if let Some(denied) = requests::recent_denial(
            &mut conn,
            environment,
            thread.node_id,
            &key,
            &arguments.reason,
            self.inner.clock.now() - Duration::hours(DENIAL_HOLD_HOURS),
        )
        .await?
        {
            let note = denied
                .decision
                .as_ref()
                .and_then(|decision| decision.get("note"))
                .and_then(Json::as_str);
            let notice = note.map_or_else(
                || format!("\"{label}\" was denied by its boundary policy."),
                |note| format!("\"{label}\" was denied: {note}"),
            );
            return Ok(done("denied", &notice, Some(denied.id)));
        }
        let live = self
            .capability_is_live(&mut conn, environment, &key)
            .await?;
        let grace = self.recreate_grace_secs(&mut conn, environment).await?;
        let effect = self.capability_effect(&mut conn, environment, &key).await?;
        let pending_notice = if live {
            crate::capabilities::PENDING_LIVE_TEXT.replace("<label>", &label)
        } else {
            said(
                messages,
                "capability_pending",
                PENDING_TEXT,
                &[("<label>", &label), ("<effect>", &effect)],
            )
        };
        if let Some(waiting) = requests::pending_for(&mut conn, environment, &key).await? {
            return Ok(done("pending", &pending_notice, Some(waiting.id)));
        }
        if self
            .pending_live_capability(&mut conn, environment, &key)
            .await?
            .is_some()
        {
            return Ok(done(
                "pending",
                &format!(
                    "\"{label}\" was approved and is being applied. Existing processes and all files are kept."
                ),
                None,
            ));
        }
        let policy = self
            .capability_policy(&mut conn, view.facts.home_node_id, &definition)
            .await?;
        let now = self.inner.clock.now();
        let request_id = request_of(thread.node_id, &call.call_ref);
        let mut audit = Entry::new(
            Actor::Agent(thread.node_id),
            orch_db::audit::action!(AuditAction::CapabilityRequested).as_str(),
            "environment",
            Some(environment),
            "An agent asked for a capability.",
        );
        audit.environment_id = Some(environment);
        audit.org_id = Some(thread.org_id);
        audit.details =
            json!({ "capability": key, "reason": arguments.reason, "decision": policy.decision });
        match policy.decision.as_str() {
            "deny" => {
                let request = self
                    .request_json(&mut conn, &view.facts, &definition, &arguments.reason)
                    .await?;
                drop(conn);
                let mut tx = self.inner.db.begin().await?;
                super::source::check(&mut tx).await?;
                requests::insert(
                    &mut tx,
                    &requests::NewCapabilityApproval {
                        id: request_id,
                        thread_id: thread.node_id,
                        turn_id: (thread.current_turn_id == Some(call.turn)).then_some(call.turn),
                        environment_id: environment,
                        title: &format!("{} asks for \"{label}\"", thread.name),
                        request: &request,
                        approver_scope: &policy.approver_scope,
                        expires_at: now + Duration::hours(REQUEST_EXPIRY_DEFAULT_HOURS),
                        at: now,
                    },
                )
                .await?;
                approval_rows::decide(
                    &mut tx,
                    request_id,
                    "denied",
                    None,
                    &json!({"approve":false,"note":null,"policy":"deny"}),
                    now,
                )
                .await?;
                append(&mut tx, &audit, &Origin::default(), now).await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                Ok(done(
                    "denied",
                    &format!("\"{label}\" is denied by policy."),
                    None,
                ))
            }
            "allow" => {
                let request = self
                    .request_json(&mut conn, &view.facts, &definition, &arguments.reason)
                    .await?;
                drop(conn);
                let mut tx = self.inner.db.begin().await?;
                let finished = {
                    let mut unit = Unit::new(&mut tx, now);
                    unit.check_tool_source().await?;
                    append(&mut *unit.conn, &audit, &Origin::default(), now).await?;
                    requests::insert(
                        &mut *unit.conn,
                        &requests::NewCapabilityApproval {
                            id: request_id,
                            thread_id: thread.node_id,
                            turn_id: (thread.current_turn_id == Some(call.turn))
                                .then_some(call.turn),
                            environment_id: environment,
                            title: &format!("{} asks for \"{label}\"", thread.name),
                            request: &request,
                            approver_scope: &policy.approver_scope,
                            expires_at: now + Duration::hours(REQUEST_EXPIRY_DEFAULT_HOURS),
                            at: now,
                        },
                    )
                    .await?;
                    approval_rows::decide(
                        &mut *unit.conn,
                        request_id,
                        "approved",
                        None,
                        &json!({"approve":true,"note":null,"policy":"allow"}),
                        now,
                    )
                    .await?;
                    self.grant_capability(
                        &mut unit,
                        &view.facts,
                        &definition,
                        Some(thread.node_id),
                        None,
                    )
                    .await?;
                    unit.finish().await?
                };
                self.commit(tx, finished).await?;
                if live && !orch_driver_contract::capabilities::gateway_given(&key) {
                    // The decision is durable before the host can give anything. Wait within
                    // the existing bounded attach budget; an offline host remains a pending
                    // application, never an answer that falsely says it is available now.
                    self.kick();
                    let until = tokio::time::Instant::now() + self.inner.config.hosting.attach_wait;
                    loop {
                        let mut conn = self.db().acquire().await?;
                        let available = env_rows::environment_facts(&mut conn, environment)
                            .await?
                            .is_some_and(|facts| facts.granted_capabilities.contains(&key));
                        if available {
                            break;
                        }
                        let applying = self
                            .pending_live_capability(&mut conn, environment, &key)
                            .await?
                            .is_some();
                        drop(conn);
                        if !applying {
                            return Ok(Refusal::new(crate::tools::definitions::Code::Unsupported,format!("The request for \"{label}\" was approved, but the driver could not apply it live. Recreate this unit before requesting it again.")).into());
                        }
                        if tokio::time::Instant::now() >= until {
                            return Ok(done(
                                "pending",
                                &format!(
                                    "\"{label}\" was approved and is being applied. Existing processes and all files are kept."
                                ),
                                Some(request_id),
                            ));
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
                let notice = if live {
                    crate::capabilities::GRANTED_LIVE_TEXT.replace("<label>", &label)
                } else {
                    said(
                        messages,
                        "capability_granted",
                        GRANTED_TEXT,
                        &[
                            ("<label>", &label),
                            ("<effect>", &effect),
                            ("<grace>", &grace.to_string()),
                        ],
                    )
                };
                Ok(done("granted", &notice, None))
            }
            _ => {
                let expiry_hours = {
                    let wanted = [crate::tools::definition_of(keys::REQUESTS_EXPIRY_HOURS)?];
                    let snapshot = Snapshot::load(
                        &mut conn,
                        Place::Node(view.facts.home_node_id),
                        Wanted::Keys(&wanted),
                    )
                    .await?;
                    crate::tools::setting::<i64>(&snapshot, keys::REQUESTS_EXPIRY_HOURS)?
                        .unwrap_or(REQUEST_EXPIRY_DEFAULT_HOURS)
                };
                let request = self
                    .request_json(&mut conn, &view.facts, &definition, &arguments.reason)
                    .await?;
                drop(conn);
                let mut tx = self.inner.db.begin().await?;
                let finished = {
                    let mut unit = Unit::new(&mut tx, now);
                    unit.check_tool_source().await?;
                    let title = format!("{} asks for \"{label}\"", thread.name);
                    let raised = requests::insert(
                        &mut *unit.conn,
                        &requests::NewCapabilityApproval {
                            id: request_id,
                            thread_id: thread.node_id,
                            turn_id: (thread.current_turn_id == Some(call.turn))
                                .then_some(call.turn),
                            environment_id: environment,
                            title: &title,
                            request: &request,
                            approver_scope: &policy.approver_scope,
                            expires_at: now + Duration::hours(expiry_hours),
                            at: now,
                        },
                    )
                    .await?;
                    if let Some(raised) = raised {
                        append(&mut *unit.conn, &audit, &Origin::default(), now).await?;
                        let approvers = self
                            .approval_recipients(&mut *unit.conn, &raised, Time::new(now))
                            .await?;
                        crate::notifications::capability_requested(
                            &mut *unit.conn,
                            thread,
                            request_id,
                            environment,
                            &label,
                            &approvers,
                            now,
                        )
                        .await?;
                    }
                    unit.finish().await?
                };
                self.commit(tx, finished).await?;
                let approvers = {
                    let mut conn = self.inner.db.acquire().await?;
                    self.capability_approvers(&mut conn, environment, &policy.approver_scope)
                        .await?
                };
                self.inner.live.approvals_changed(&approvers);
                Ok(done("pending", &pending_notice, Some(request_id)))
            }
        }
    }

    /// The request as an approval holds it: what is asked, who asks for what, and what granting it would do.
    async fn request_json(
        &self,
        conn: &mut orch_db::Conn,
        facts: &env_rows::EnvironmentFacts,
        definition: &env_rows::DefinitionRow,
        reason: &str,
    ) -> Result<Json, EngineError> {
        let environment =
            orch_db::queries::environments::thread_engine::environment(conn, facts.id)
                .await?
                .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        let environment_ref = crate::views::environment_ref_json(
            conn,
            &environment,
            orch_types::Time::new(self.inner.clock.now()),
        )
        .await?;
        let host = match self.capability_host(conn, facts.id).await? {
            Some((host, _)) => {
                let name: Option<String> = sqlx_name_of_host(conn, host).await?;
                json!({ "id": host, "name": name.clone().unwrap_or_default(),
                        "address": crate::views::address_json(conn, None, None, &name.unwrap_or_default()).await? })
            }
            None => json!({ "id": Id::nil(), "name": "", "address": "" }),
        };
        let effect = self
            .capability_effect(conn, facts.id, &definition.key)
            .await?;
        // What the runtime the environment is under says of this capability: how risky it is there, and what works
        // differently for it.
        let declared = orch_db::queries::environment_facts::of(conn, facts.id)
            .await?
            .and_then(|machine| machine.descriptor);
        let (risk, limits) = declared.as_ref().map_or_else(
            || (definition.risk.clone(), Vec::new()),
            |declared| {
                crate::isolation::capability_words(declared, &definition.key, &definition.risk)
            },
        );
        let mut consequences = vec![effect, definition.description.clone()];
        consequences.extend(limits);
        Ok(json!({
            "capability": definition.key,
            "label": definition.label,
            "risk": risk,
            "reason": reason,
            "environment": environment_ref,
            "host": host,
            "consequences": consequences,
        }))
    }
}

/// The name of a host.
async fn sqlx_name_of_host(
    conn: &mut orch_db::Conn,
    host: Id,
) -> Result<Option<String>, EngineError> {
    Ok(orch_db::queries::threads::tools::host_name(conn, host).await?)
}

/// The approval that a call raised, for those who look for it by the call.
#[must_use]
pub fn request_of(thread: Id, call_ref: &str) -> Id {
    ids::derived(
        "capability.approval",
        &[thread.as_bytes().as_slice(), call_ref.as_bytes()],
    )
}

/// Reads back an approval raised by a call.
pub async fn raised_by(
    conn: &mut orch_db::Conn,
    thread: Id,
    call_ref: &str,
) -> Result<Option<approval_rows::ApprovalRow>, EngineError> {
    Ok(approval_rows::approval(conn, request_of(thread, call_ref)).await?)
}
