//! Displays of an environment and single-use tickets to watch or work in them.

use crate::operations::{
    FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target, VariantSpec as Variant,
};
use orch_access_contract::key_scope::KeyAction::{Manage, Read};
use orch_access_contract::key_scope::KeyTargetKind::Object;
use orch_access_contract::permissions::ObjectKind as K;

use crate::browsers::{BrowserOrigin, BrowserState, BrowsersOff, EnvironmentBrowsersLimit};
use crate::environment::{EnvironmentRef, EnvironmentSupportVnc, ThroughThreadQuery};
use crate::nodes::NodeRef;
use crate::routes::Access::{Permission, Public, Rule};
use crate::users::UserRef;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The binary RFB socket protocol.
pub const DISPLAY_SUBPROTOCOL: &str = "orch.display.v1";
/// Display tickets have the same thirty-second lifetime as browser tickets.
pub const DISPLAY_TICKET_SECONDS: u32 = 30;
/// Display socket close reasons use the application close code.
pub const DISPLAY_CLOSE_CODE: u16 = 4000;

/// A virtual screen or an agent's announced VNC server, without its credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Display {
    /// Its identifier.
    pub id: Id,
    /// Where it runs.
    pub environment: EnvironmentRef,
    /// Its listed name.
    pub name: String,
    /// Whether Orch started its server.
    pub origin: BrowserOrigin,
    /// Its human opener, or null for an agent.
    pub opened_by: Option<UserRef>,
    /// The associated thread, where visible.
    pub thread: Option<NodeRef>,
    /// Its local endpoint, only for a holder of Manage.
    pub endpoint: Option<String>,
    /// The X address to give a windowed program.
    pub x_display: Option<String>,
    /// Its dimensions, where known.
    pub geometry: Option<String>,
    /// The current viewer count.
    pub watching: i64,
    /// People recently working in it.
    pub driving: Vec<UserRef>,
    /// The agent's short purpose line.
    pub note: Option<String>,
    /// Browsers opened with a window on this display.
    pub browsers: Vec<Id>,
    /// Its lifecycle state.
    pub state: BrowserState,
    /// Why it ended.
    pub close_reason: Option<String>,
    /// When it opened.
    pub opened_at: Time,
    /// Its last use.
    pub last_used_at: Time,
    /// When it ended.
    pub closed_at: Option<Time>,
}

/// The current displays, discovered support and policy gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentDisplays {
    /// Why the door is off, or null.
    pub off: Option<BrowsersOff>,
    /// The installed display programs.
    pub support: EnvironmentSupportVnc,
    /// Open displays followed by those closed within a day.
    pub displays: Vec<Display>,
    /// Owned live displays and the resolved limit.
    pub limit: EnvironmentBrowsersLimit,
}

/// Optionally refreshes installed support before listing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDisplayListQuery {
    /// Look again before answering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<bool>,
}

/// Opens a virtual screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDisplayCreateBody {
    /// Its listed name, or its X display when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its dimensions, or the environment's setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<String>,
}

/// Requests watching, or also working with Manage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisplayTicketCreateBody {
    /// Allows input as well as watching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<bool>,
}

/// A short single-use socket credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayTicketCreateReply {
    /// The credential.
    pub ticket: String,
    /// Its expiry.
    pub ticket_expires_at: Time,
}

/// The credential supplied to a display socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisplayAttachQuery {
    /// The single-use ticket.
    pub ticket: String,
}

/// The completed close without the existing display's private metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayCloseReceipt {
    /// This acknowledgement carries no existing session content.
    pub projection: crate::common::ReceiptProjection,
    /// The display named by this close.
    pub display_id: Id,
    /// The actual environment whose close operation was admitted.
    pub environment_id: Id,
    /// The row is closed, including an idempotent repeated close.
    pub closed: bool,
}

crate::common::full_or_receipt! {
    /// The ordinary full projection or this same close operation's restricted receipt.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
    pub enum DisplayCloseResponse {
        /// Existing content when this operation currently admits its Read facet.
        Full(Display),
        /// Identifiers and safe completed outcome only.
        Receipt(DisplayCloseReceipt),
    }
}

/// The actual accepted display without private current session metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayCreateReceipt {
    /// This acknowledgement contains no full session projection.
    pub projection: crate::common::ReceiptProjection,
    /// The actual native reservation.
    pub display_id: Id,
    /// The actual native environment.
    pub environment_id: Id,
}
crate::common::full_or_receipt! {
    /// The native first/current session or its accepted identity receipt.
    #[derive(Debug,Clone,PartialEq,Serialize,Deserialize,JsonSchema)]
    pub enum DisplayCreateResponse {
        /// Same native session metadata, under this operation's current Read.
        Full(Display),
        /// No session metadata is disclosed and no service is opened twice.
        Receipt(DisplayCreateReceipt),
    }
}

crate::common::full_or_receipt! {
    /// A new once-only display ticket or its already accepted identity.
    #[derive(Debug,Clone,PartialEq,Serialize,Deserialize,JsonSchema)]
    pub enum DisplayTicketCreateResponse {
        /// The unchanged first native credential.
        Full(DisplayTicketCreateReply),
        /// No secret is stored or returned again.
        Receipt(crate::common::TicketCreateReceipt),
    }
}

crate::endpoints! {
    /// Lists displays and optionally rediscovers installed programs.
    EnvironmentDisplayList: GET "/environments/{id}/displays", Permission("vnc.view"), query EnvironmentDisplayListQuery, none, json EnvironmentDisplays, 200;
    /// Opens a virtual screen for programs with windows.
    EnvironmentDisplayCreate: POST "/environments/{id}/displays", Permission("vnc.manage"), query ThroughThreadQuery, json EnvironmentDisplayCreateBody, json DisplayCreateResponse, 201,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage,&[Object(K::Environment)],Target::PermissionObject),
            Facet::response_content(&[Object(K::Environment)],Target::PermissionObject),
            Facet::response_content(&[Object(K::Thread)],Target::ResolvedCollection("close_threads")),
        ]));
    /// Stops an owned virtual screen or lets go of an announced server.
    DisplayClose: DELETE "/displays/{display_id}", Rule("vnc.manage on its environment"), none, none, json DisplayCloseResponse, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage, &[Object(K::Environment)], Target::Resolved("session_environment")),
            Facet::response_content(&[Object(K::Environment)], Target::Resolved("session_environment")),
            Facet::response_content(&[Object(K::Thread)], Target::ResolvedCollection("close_threads")),
        ]));
    /// Gives a ticket to watch or, with Manage, work in a display.
    DisplayTicketCreate: POST "/displays/{display_id}/ticket", Rule("vnc.view on its environment; control also needs vnc.manage"), none, json DisplayTicketCreateBody, json DisplayTicketCreateResponse, 200,
        operation Op::receipt(&Op::variants(&[
            Variant {
                name: "view",
                selector: "control is false",
                facets: &[Facet::required(
                    Read,
                    &[Object(K::Environment)],
                    Target::Resolved("session_environment"),
                )],
            },
            Variant {
                name: "control",
                selector: "control is true",
                facets: &[
                    Facet::required(
                        Read,
                        &[Object(K::Environment)],
                        Target::Resolved("session_environment"),
                    ),
                    Facet::required(
                        Manage,
                        &[Object(K::Environment)],
                        Target::Resolved("session_environment"),
                    ),
                ],
            },
        ]));
    /// Carries a display's RFB connection without a viewer password.
    DisplayAttach: GET "/ws/displays/{display_id}", Public, query DisplayAttachQuery, none, raw, 101,
        operation Op::protocol();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn a_person_opens_a_screen_without_supplying_a_server_or_password() {
        let body: EnvironmentDisplayCreateBody = serde_json::from_value(json!({})).unwrap();
        assert_eq!(body.geometry, None);
        assert!(
            serde_json::from_value::<EnvironmentDisplayCreateBody>(json!({"password":"private"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<EnvironmentDisplayCreateBody>(
                json!({"endpoint":"remote:5900"})
            )
            .is_err()
        );
    }
}
