//! Shells that are their opener's alone: sessions attached or left running, the ticket that takes one to another screen, the choice of which to close at a person's limit, and the diagnostic terminal.
//!
//! A shell is its person's, and it stays while they are away: closing the tab
//! detaches it and ends nothing, and its person attaches again with a new
//! ticket and finds the screen as it was. A ticket is the credential of a
//! WebSocket, because a browser cannot set a header on one; it is used once and
//! lasts [`TERMINAL_TICKET_SECONDS`]. Where the stream itself is described is
//! the realtime contract, not here.

use crate::operations::{
    FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target, VariantSpec as Variant,
};
use orch_access_contract::key_scope::KeyAction::{Manage, Read};
use orch_access_contract::key_scope::KeyTargetKind::Object;
use orch_access_contract::permissions::ObjectKind as K;

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::Page;
use crate::environment::ThroughThreadQuery;
use crate::routes::Access::{Permission, Public, Rule};
use crate::users::UserRef;

/// The WebSocket subprotocol both terminal streams speak.
pub const TERMINAL_SUBPROTOCOL: &str = "orch.terminal.v1";
/// How long a ticket stays good, in seconds. Short, because it is single-use and travels in an
/// address.
pub const TERMINAL_TICKET_SECONDS: u32 = 30;
/// The most bytes of what a shell printed that are kept when it closes.
pub const TERMINAL_RECORDING_MAX_BYTES: i64 = 256 * 1024;

/// Where a shell stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    /// Being started.
    Opening,
    /// Running.
    Open,
    /// Ended.
    Closed,
}

/// A shell in an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalSession {
    /// The session's identifier.
    pub id: Id,
    /// The thread it was opened from, when it was opened through one.
    pub thread_id: Option<Id>,
    /// The environment it runs in.
    pub environment_id: Id,
    /// Whose it is: only they attach to it.
    pub opened_by: UserRef,
    /// The shell program.
    pub shell: String,
    /// The directory it started in.
    pub cwd: String,
    /// Its width, in columns.
    pub cols: u16,
    /// Its height, in rows.
    pub rows: u16,
    /// Where it stands.
    pub state: TerminalState,
    /// Why it ended: it exited, it was closed, its environment stopped or was recreated, its
    /// person lost a gate, or nobody was attached for too long.
    pub close_reason: Option<String>,
    /// What it exited with.
    pub exit_code: Option<i64>,
    /// Whether its person has it on a screen now; false means it runs on by itself.
    pub attached: bool,
    /// Since when nobody has been attached.
    pub detached_at: Option<Time>,
    /// When it will be closed for that.
    pub closes_at: Option<Time>,
    /// Whether a command is running in it, as its machine last reported.
    pub busy: bool,
    /// Whether the end of what it prints is kept when it closes.
    pub recorded: bool,
    /// When it was opened.
    pub opened_at: Time,
    /// When something last went in or out.
    pub last_io_at: Time,
    /// When it ended.
    pub closed_at: Option<Time>,
}

/// The single-use credential by which a WebSocket attaches to a terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalTicket {
    /// The ticket, to be sent as `ticket` in the query of the WebSocket's address.
    pub ticket: String,
    /// When it stops being good.
    pub ticket_expires_at: Time,
}

/// Which of the caller's shells to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalListState {
    /// Those that run.
    Open,
    /// Those that ended.
    Closed,
}

/// The caller's own shells in an environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentTerminalListQuery {
    /// The thread the caller works beside. It gives nothing: what is needed is held on the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
    /// Only those in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<TerminalListState>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Opens a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentTerminalCreateBody {
    /// Its width, in columns.
    pub cols: u16,
    /// Its height, in rows.
    pub rows: u16,
    /// Where it starts, relative to the workspace root; the thread's own directory for the agent,
    /// or the root, when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Shells of the caller's own in that environment to close first, in the same step. This is
    /// how a new shell is opened at the limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<Vec<Id>>,
}

/// A shell that was opened, with the ticket to attach to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentTerminalCreateReply {
    /// The shell.
    pub session: TerminalSession,
    /// The ticket, to be sent as `ticket` in the query of the WebSocket's address.
    pub ticket: String,
    /// When it stops being good.
    pub ticket_expires_at: Time,
    /// The shells that were closed to make room for it.
    pub closed: Vec<Id>,
}

/// The actual created shell, without metadata or a new control ticket after disclosure/authority loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalCreateReceipt {
    /// The established restricted-success discriminator.
    pub projection: crate::common::ReceiptProjection,
    /// The actual created native resource.
    pub terminal_id: Id,
    /// Its actual governing environment.
    pub environment_id: Id,
    /// The actual owned shells closed by this accepted creation.
    pub closed: Vec<Id>,
}

/// The unchanged creation result, or a successful receipt after the native effect completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EnvironmentTerminalCreateResponse {
    /// The completed act without existing metadata or a new credential.
    Receipt(TerminalCreateReceipt),
    /// The original native full result and live control ticket.
    Full(Box<EnvironmentTerminalCreateReply>),
}
impl EnvironmentTerminalCreateResponse {
    /// Extracts a real full native result without fabricating metadata/tickets for a receipt.
    pub fn into_full(self) -> Result<EnvironmentTerminalCreateReply, TerminalCreateReceipt> {
        match self {
            Self::Full(value) => Ok(*value),
            Self::Receipt(value) => Err(value),
        }
    }
}

/// Asks for a ticket for the agent's own terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentTerminalTicketCreateBody {
    /// Whether the ticket lets its holder type into that terminal; without it they only watch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<bool>,
}

/// The accepted close, without the shell's existing directory/program/output metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalCloseReceipt {
    /// The established restricted-success discriminator.
    pub projection: crate::common::ReceiptProjection,
    /// The actual shell which is closed or was already closed.
    pub terminal_id: Id,
    /// Its actual governing environment at acceptance.
    pub environment_id: Id,
    /// The native close is complete, including an idempotent close.
    pub closed: bool,
}

/// Native closed metadata when this operation's Read permits it, or its successful receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TerminalCloseResponse {
    /// The successful act with no old shell content.
    Receipt(TerminalCloseReceipt),
    /// The unchanged full closed-session wire value.
    Full(Box<TerminalSession>),
}

impl TerminalCloseResponse {
    /// Extracts actual native metadata without interpreting a receipt as a shell session.
    pub fn into_full(self) -> Result<TerminalSession, TerminalCloseReceipt> {
        match self {
            Self::Full(value) => Ok(*value),
            Self::Receipt(value) => Err(value),
        }
    }
}

/// The ticket by which a WebSocket attaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalAttachQuery {
    /// The ticket.
    pub ticket: String,
}

/// Why a shell is refused although the caller holds the permission; no permission lifts these.
/// It is the `reason` of a `terminal.not_allowed` error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalNotAllowedReason {
    /// A setting forbids shells here.
    DisabledByPolicy,
    /// The environment's own switch for shells is off.
    EnvironmentSwitchOff,
    /// The local policy of the machine it is on forbids shells.
    MachinePolicy,
    /// The environment is not reachable.
    EnvironmentOffline,
}

crate::endpoints! {
    /// Lists the caller's own shells in an environment, attached or not, newest first; whoever manages the environment sees everyone's.
    EnvironmentTerminalList: GET "/environments/{id}/terminals", Permission("terminal.manage"), query EnvironmentTerminalListQuery, none, json Page<TerminalSession>, 200;
    /// Opens a shell in an environment, closing some of the caller's own first when they are at the limit, and gives the ticket to attach to it.
    EnvironmentTerminalCreate: POST "/environments/{id}/terminals", Rule("terminal.manage on the environment; and the conditions the settings, the environment and its machine set for a shell"), query ThroughThreadQuery, json EnvironmentTerminalCreateBody, json EnvironmentTerminalCreateResponse, 201,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Read, &[Object(K::Environment)], Target::Path("id")),
            Facet::required(Manage, &[Object(K::Environment)], Target::Path("id")),
        ]));
    /// Attaches a WebSocket to a shell; a shell has one screen at a time, so an attachment elsewhere is ended and told so.
    TerminalAttach: GET "/ws/terminals/{terminal_id}", Public, query TerminalAttachQuery, none, raw, 101,
        operation Op::protocol();
    /// Gives a new ticket to attach to a shell again, from this browser or another.
    TerminalTicketCreate: POST "/terminals/{terminal_id}/ticket", Rule("the opener, who still passes the gates it was opened under"), none, none, json TerminalTicket, 200,
        operation Op::facets(&[
            Facet::required(
                Read,
                &[Object(K::Environment)],
                Target::Resolved("terminal_environment"),
            ),
            Facet::required(
                Manage,
                &[Object(K::Environment)],
                Target::Resolved("terminal_environment"),
            ),
        ]);
    /// Closes a shell.
    TerminalClose: DELETE "/terminals/{terminal_id}", Rule("the opener, or environment.manage"), none, none, json TerminalCloseResponse, 200,
        operation Op::receipt(&Op::returning(
            Manage,
            &[Object(K::Environment)],
            Target::Resolved("terminal_environment"),
        ));
    /// Gives a ticket for the agent's own terminal of a thread, for watching it or, with control, typing into it.
    AgentTerminalTicketCreate: POST "/threads/{id}/agent-terminal/ticket", Rule("terminal.view on the thread's environment; a ticket with control needs terminal.manage there"), none, json AgentTerminalTicketCreateBody, json TerminalTicket, 200,
        operation Op::variants(&[
            Variant {
                name: "view",
                selector: "control=false",
                facets: &[Facet::required(
                    Read,
                    &[Object(K::Environment)],
                    Target::Resolved("thread_environment"),
                )],
            },
            Variant {
                name: "control",
                selector: "control=true",
                facets: &[
                    Facet::required(
                        Read,
                        &[Object(K::Environment)],
                        Target::Resolved("thread_environment"),
                    ),
                    Facet::required(
                        Manage,
                        &[Object(K::Environment)],
                        Target::Resolved("thread_environment"),
                    ),
                ],
            },
        ]);
    /// Attaches a WebSocket to the agent's own terminal of a thread; input is accepted only on a ticket with control.
    AgentTerminalAttach: GET "/ws/agent-terminals/{thread_id}", Public, query TerminalAttachQuery, none, raw, 101,
        operation Op::protocol();
    /// Reads what a shell last printed, as it was kept when it closed.
    TerminalRecordingGet: GET "/terminals/{terminal_id}/recording", Rule("the opener, environment.manage, or audit.view on the thread it was opened from"), none, none, raw, 200,
        operation Op::facets(&[
            Facet::required(Read, &[Object(K::Environment)], Target::Resolved("terminal_environment")),
            Facet::required(Read, &[Object(K::Thread)], Target::ResolvedCollection("recording_audit_thread")),
        ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session() -> serde_json::Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "thread_id": null,
            "environment_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c",
            "opened_by": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d", "kind": "human", "username": "bo", "display_name": "Bo", "avatar": { "text": "B", "color": 2 } },
            "shell": "/bin/bash", "cwd": "/work", "cols": 120, "rows": 32,
            "state": "open", "close_reason": null, "exit_code": null,
            "attached": false, "detached_at": "2026-10-08T17:10:00.000Z", "closes_at": "2026-10-09T01:10:00.000Z",
            "busy": true, "recorded": true,
            "opened_at": "2026-10-08T17:04:05.123Z", "last_io_at": "2026-10-08T17:09:59.000Z", "closed_at": null
        })
    }

    #[test]
    fn a_shell_left_running_says_when_it_will_be_closed() {
        let shell: TerminalSession = serde_json::from_value(session()).unwrap();
        assert_eq!(shell.state, TerminalState::Open);
        assert!(!shell.attached && shell.busy);
        assert!(shell.closes_at.is_some());
        assert_eq!(serde_json::to_value(&shell).unwrap(), session());
    }

    #[test]
    fn opening_a_shell_at_the_limit_closes_some_of_the_callers_own_in_the_same_step() {
        let body: EnvironmentTerminalCreateBody = serde_json::from_value(json!({
            "cols": 80, "rows": 24, "close": ["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b"]
        }))
        .unwrap();
        assert_eq!(body.close.as_ref().map(Vec::len), Some(1));
        assert_eq!(body.cwd, None);
        assert!(
            serde_json::from_value::<EnvironmentTerminalCreateBody>(json!({ "cols": 80 })).is_err(),
            "a shell needs a size"
        );
        let reply: EnvironmentTerminalCreateReply = serde_json::from_value(json!({
            "session": session(), "ticket": "t0k3n", "ticket_expires_at": "2026-10-08T17:04:35.123Z",
            "closed": ["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b"]
        }))
        .unwrap();
        assert_eq!(reply.closed.len(), 1);
    }

    #[test]
    fn the_reasons_a_shell_is_refused_are_written_as_the_error_carries_them() {
        assert_eq!(
            serde_json::to_value([
                TerminalNotAllowedReason::DisabledByPolicy,
                TerminalNotAllowedReason::EnvironmentSwitchOff,
                TerminalNotAllowedReason::MachinePolicy,
                TerminalNotAllowedReason::EnvironmentOffline,
            ])
            .unwrap(),
            json!([
                "disabled_by_policy",
                "environment_switch_off",
                "machine_policy",
                "environment_offline"
            ])
        );
    }
}
