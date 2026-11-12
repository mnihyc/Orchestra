//! Personal, read-only conversations about one fixed root organization.
//!
//! A conversation is backed by a hidden thread. Its storage home and the
//! organization governing its tools, inspection and retention are distinct.
//! Only its owner sends messages; root-organization administrators may inspect
//! and export it through their ordinary explicit administrative opening.

use crate::operations::{FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target};
use crate::routes::Access::Rule;
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_types::{Id, Json, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A selected API model; subscription-only providers cannot serve Ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskModelIdentity {
    pub provider_id: Id,
    pub model: String,
}

/// A conversation's model identity and its optional current parameter choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskModel {
    /// The provider whose runtime keeps its credential.
    pub provider_id: Id,
    /// The enabled routing name.
    pub model: String,
    /// Choices within the provider's effective constraints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
}

impl From<AskModelIdentity> for AskModel {
    fn from(identity: AskModelIdentity) -> Self {
        Self {
            provider_id: identity.provider_id,
            model: identity.model,
            params: None,
        }
    }
}

/// A source in the answer, rendered with the current reader's ordinary access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskCitation {
    /// Opaque source reference, suitable for a later `open` call.
    pub reference: String,
    /// The canonical Orch address; never an arbitrary external fetch target.
    pub url: String,
    /// A current readable label, absent when access has ended.
    pub title: Option<String>,
}

/// The conversation's current lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AskState {
    /// Ready to receive another message.
    Ready,
    /// One turn is active; a second input cannot start another simultaneous turn.
    Running,
    /// The owner can no longer enter its root organization.
    Closed,
    /// A permanent thread was created from its history.
    Kept,
}

/// Effective capabilities for this opening, including explicit admin inspection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskCan {
    /// Send input as the owner within the original root organization.
    pub message: bool,
    /// Stop the owner's running turn.
    pub stop: bool,
    /// Create a Private permanent thread in an authorized destination.
    pub keep: bool,
    /// Delete this conversation.
    pub delete: bool,
    /// Export through the ordinary current read/export authority.
    pub export: bool,
}

/// History row and conversation header; no conversation text is embedded here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskConversation {
    /// The hidden thread's stable identity, also the conversation identity.
    pub id: Id,
    /// The person who owns this conversation.
    pub user_id: Id,
    /// Its immutable knowledge and governance boundary.
    pub root_org_id: Id,
    /// Its storage home, usually the owner's personal organization.
    pub home_org_id: Id,
    /// A readable title.
    pub title: String,
    /// Current lifecycle.
    pub state: AskState,
    /// The person's model preference; null follows today's effective default on each turn.
    pub model: Option<AskModel>,
    /// The effective payer, independently of usage attribution to the owner.
    pub payer_id: Option<Id>,
    /// Creation time.
    pub created_at: Time,
    /// Last accepted message time, from which retention is counted.
    pub last_message_at: Time,
    /// Permanent purge time, with no additional soft-delete window.
    pub expires_at: Time,
    /// Retention after applying the root organization's minimum and maximum.
    pub retention_days: u32,
    /// Whether the owner chose to show the hidden thread in ordinary listings.
    pub shown: bool,
    /// Administrators of this root organization can inspect and export it.
    pub admin_inspectable: bool,
    /// True for an audited administrative opening, which never permits input.
    pub inspecting: bool,
    /// Current actions for this opening.
    pub can: AskCan,
    /// Last committed event sequence.
    pub seq: Seq,
}

/// Begin a conversation. The server resolves and freezes the page's root org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskCreateBody {
    /// The root org selected in the rail when the page is outside every org.
    #[serde(default)]
    pub root_org_id: Option<Id>,
    /// An Orch reference or accepted address for the page in view.
    #[serde(default)]
    pub context: Option<String>,
    /// An allowed model; omitted to use today's effective default.
    #[serde(default)]
    pub model: Option<AskModel>,
    /// Owner's retention choice, clamped to the root org's bounds.
    #[serde(default)]
    pub retention_days: Option<u32>,
}

/// Owner's history, paged across orgs without mixing their conversations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskListQuery {
    /// Inspect one root organization's conversations through explicit As admin mode.
    #[serde(default)]
    pub inspect: bool,
    /// Limit history to this fixed root org when present.
    #[serde(default)]
    pub root_org_id: Option<Id>,
    /// Opaque server pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Fifty by default, at most two hundred.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Bounded conversation history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskListReply {
    /// Current readable headers.
    pub items: Vec<AskConversation>,
    /// Next history page.
    pub next_cursor: Option<String>,
}

/// A visible replacement for the oldest context, without deleting its original messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskSummary {
    /// Original history through this sequence is represented by the summary.
    pub through: Seq,
    /// The conversation's own model that wrote the summary, under its ordinary Ask payer.
    pub model: AskModel,
}

/// One user or assistant message, retained in the hidden thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskMessage {
    /// The ordinary thread item identity.
    pub id: Id,
    /// Its turn.
    pub turn_id: Id,
    /// Person or assistant; tool material is exposed as sources, not instructions.
    pub role: AskRole,
    /// Message text and attachment references.
    pub content: Vec<crate::threads::ContentPart>,
    /// Readable canonical source links.
    pub citations: Vec<AskCitation>,
    /// Show as an expandable “Earlier messages summarized” item; text remains in `content`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<AskSummary>,
    /// Whether this message is complete.
    pub complete: bool,
    /// Acceptance time.
    pub created_at: Time,
}

/// The two visible message authors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AskRole {
    /// The conversation's owner.
    User,
    /// The built-in program's answer.
    Assistant,
}

/// Read the current conversation and a bounded page of its messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskGetReply {
    /// Current header and effective rights.
    pub conversation: AskConversation,
    /// Messages in ascending event order.
    pub messages: Vec<AskMessage>,
    /// A previous message page, if any.
    pub previous_cursor: Option<String>,
}

/// History paging without altering the conversation's org or principal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskGetQuery {
    /// A prior message page.
    #[serde(default)]
    pub before: Option<String>,
    /// Fifty by default, at most two hundred.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Submit one idempotent input. Current settings resolve model and payer anew.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskMessageBody {
    /// Reusing this value for the same sender/thread returns the same receipt.
    pub client_id: String,
    /// Ordinary composer content and references.
    pub content: Vec<crate::threads::ContentPart>,
    /// The page now in view, re-read under this turn's admitted principal.
    #[serde(default)]
    pub context: Option<String>,
    /// Omission keeps the saved preference; null chooses today's default;
    /// an object selects that model within today's allowed lists.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<Option<AskModel>>,
}

/// Content-free acknowledgement, safe for a write-only operation grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskReceipt {
    /// The conversation.
    pub conversation_id: Id,
    /// The accepted turn, when this act starts or stops one.
    pub turn_id: Option<Id>,
    /// Last committed event sequence.
    pub seq: Seq,
}

/// Resume after this event; a missing value starts after the current snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskEventsQuery {
    /// Last applied event, also accepted as the SSE Last-Event-ID header.
    #[serde(default)]
    pub after: Option<Seq>,
}

/// One ordered server-sent event; SSE event name is `ask`, id is `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskEvent {
    /// Conversation whose current read permission is checked before delivery.
    pub conversation_id: Id,
    /// Monotonic within this conversation; duplicates are ignored by clients.
    pub seq: Seq,
    /// The changed data.
    pub data: AskEventData,
}

/// Streaming and committed changes use one discriminated event representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskEventData {
    /// The current header changed, including after Stop or Keep.
    Conversation { conversation: Box<AskConversation> },
    /// A new message, or the final replacement after streaming.
    Message { message: Box<AskMessage> },
    /// Text appended at a Unicode-scalar offset; a gap requires GET and resubscribe.
    Delta {
        message_id: Id,
        offset: i64,
        text: String,
    },
    /// A tool started or finished; only a readable label and citations are exposed.
    Tool {
        call_id: String,
        name: String,
        complete: bool,
        citations: Vec<AskCitation>,
    },
    /// Safe notice, such as a model change or a withdrawn earlier tool result.
    Notice { code: String, message: String },
    /// Safe turn failure; no upstream credentials or raw response bodies.
    Error {
        code: String,
        message: String,
        /// Selected model when context fitting refused this question locally.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Replay is unavailable; read the current snapshot before reconnecting.
    Reset,
    /// Permanent deletion; the stream then closes.
    Deleted,
}

/// Preserve or fork a built-in conversation as a Private thread in an admitted organization.
/// Copying out additionally requires its stored and knowledge roots' native copy and Export controls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskKeepBody {
    /// Explicit existing topic, overriding the default place choice.
    #[serde(default)]
    pub topic_id: Option<Id>,
    /// Authorized project in which to create a topic if none was chosen.
    #[serde(default)]
    pub project_id: Option<Id>,
    /// Optional title for the new thread.
    #[serde(default)]
    pub title: Option<String>,
    /// Copy history through this visible finished turn, inclusive; absent copies all history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_turn: Option<i64>,
}

/// Identity-only result of Keep as thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskKeepReply {
    /// Source conversation; a temporary source is now kept, a permanent source remains open.
    pub conversation_id: Id,
    /// New Private thread with this conversation's history.
    pub thread_id: Id,
    /// Its authorized destination topic.
    pub topic_id: Id,
}

/// Personal display and retention choices; the governing org is immutable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUpdateBody {
    /// Show this hidden thread in ordinary authorized listings.
    pub shown: Option<bool>,
    /// Chosen days, clamped to the current root org bounds.
    pub retention_days: Option<u32>,
}

/// Read model choices at a root org without thread-creation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskModelsQuery {
    /// The conversation's fixed root organization.
    pub root_org_id: Id,
}

/// A native API-model choice, with all exclusions kept visible and explained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskModelOption {
    /// The exact effective policy-list identity; parameters belong to the person.
    pub selection: AskModelIdentity,
    /// Ordinary readable provider identity and address.
    pub provider: Option<crate::resources::ProviderRef>,
    /// Existing native model capabilities, constraints and allowance.
    pub model: Option<crate::settings::ModelOption>,
    /// Resolved wire-option schema for the generation interface Ask will use.
    /// It comes from the native endpoint/group/model constraint composer; absent
    /// when the configured model or a generation interface is unavailable.
    pub request_option_schema: Option<Json>,
    /// This configured entry is usable by this person at this root.
    pub allowed: bool,
    /// The first usable entry in the configured order.
    pub is_default: bool,
    /// Provider-level exclusion, beside the model's existing typed reason.
    pub unavailable_reason: Option<String>,
}

/// Why the root's configured Ask list cannot answer a question for this person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AskModelsBlockedReason {
    NotConfigured,
    Empty,
    NoUsableModels,
    ExcludedByPolicy,
}

/// Current native model, Use and policy projection for the root-scoped picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskModelsReply {
    pub root_org_id: Id,
    pub default_model: Option<AskModelIdentity>,
    pub models: Vec<AskModelOption>,
    /// Null when at least one configured entry is usable.
    pub blocked_reason: Option<AskModelsBlockedReason>,
    /// The normal settings-view permission and lock result for ask.models here.
    pub writable: bool,
}

/// A completed private upload linked to this conversation, with no workspace.
pub type AskAttachmentBody = crate::chats::ChatAttachmentBlobBody;
/// The ordinary blob attachment receipt, with the caller-supplied name.
pub type AskAttachmentReply = crate::chats::ChatAttachmentCreateReply;

/// Reuse the canonical saved-byte source roles and custody rules.
pub const fn attachment_facets() -> [Facet; 6] {
    let mut facets = crate::chat_sources::attachment_facets();
    facets[0] = Facet::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        Target::TypedPath {
            parameter: "id",
            kind: KeyTargetKind::Object(ObjectKind::Thread),
        },
    );
    facets
}

crate::endpoints! {
    /// Links a finished private upload to this conversation without an environment.
    AskAttachmentCreate: POST "/ask/conversations/{id}/attachments", Rule("owner with current root access and the native saved-byte source authority"), none, json AskAttachmentBody, json AskAttachmentReply, 201,
        operation Op::facets(&attachment_facets());
    /// Reads a private conversation attachment as its owner or inspecting root administrator.
    AskAttachmentGet: GET "/ask/conversations/{id}/attachments/{hash}", Rule("same owner or root org inspection authority as AskGet, and exact attached hash"), query crate::chats::ChatBlobGetQuery, none, raw, 200,
        operation Op::facets(&[Facet::required(KeyAction::Read,&[KeyTargetKind::Object(ObjectKind::Thread)],Target::TypedPath {parameter:"id",kind:KeyTargetKind::Object(ObjectKind::Thread)})]);
    /// Lists the person's current API-model choices for a root-org Ask.
    AskModelsGet: GET "/ask/models", Rule("root org view and existing model, provider Use and policy resolution"), query AskModelsQuery, none, json AskModelsReply, 200,
        operation Op::facets(&[
            Facet::required(KeyAction::Read,&[KeyTargetKind::Object(ObjectKind::Org)],Target::Resolved("ask_model_root")),
            Facet::response_content(&[KeyTargetKind::Object(ObjectKind::Provider)],Target::ResolvedCollection("ask_model_providers"))
        ]);
    /// Begins a personal conversation with a fixed root organization.
    AskCreate: POST "/ask/conversations", Rule("owner creates in personal home with root org access"), none, json AskCreateBody, json AskReceipt, 201,
        operation Op::receipt(&Op::facets(&[Facet::required(KeyAction::Write, &[KeyTargetKind::Identity(KeyIdentityKind::Account)], Target::CurrentAccount)]));
    /// Lists the caller's readable personal conversations.
    AskList: GET "/ask/conversations", Rule("owner history under current root org access"), query AskListQuery, none, json AskListReply, 200,
        operation Op::facets(&[
            Facet::required(KeyAction::Read, &[KeyTargetKind::Identity(KeyIdentityKind::Account)], Target::CurrentAccount),
            Facet::response_content(&[KeyTargetKind::Object(ObjectKind::Org)],Target::ResolvedCollection("ask_roots"))
        ]);
    /// Reads a personal conversation as its owner or an audited root org administrator.
    AskGet: GET "/ask/conversations/{id}", Rule("owner or explicit root org admin inspection"), query AskGetQuery, none, json AskGetReply, 200,
        operation Op::facets(&[Facet::required(KeyAction::Read, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) })]);
    /// Submits to the built-in dialogue as the admitted person, including a permanent kept thread.
    AskMessageCreate: POST "/ask/conversations/{id}/messages", Rule("temporary owner or ordinary permanent-thread View and Interact, with current root access"), none, json AskMessageBody, json AskReceipt, 202,
        operation Op::facets(&[
            Facet::required(KeyAction::Write, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) }),
            Facet::required(KeyAction::Read,&[KeyTargetKind::Object(ObjectKind::Thread)],Target::ResolvedCollection("ask_attachment_sources")),
            Facet::required(KeyAction::Read,&[KeyTargetKind::Object(ObjectKind::Thread)],Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) })
        ]);
    /// Streams ordered Ask events under current reader authority.
    AskEvents: GET "/ask/conversations/{id}/events", Rule("same read authority as AskGet throughout delivery"), query AskEventsQuery, none, raw, 200,
        operation Op::facets(&[Facet::required(KeyAction::Read, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) })]);
    /// Stops the owner's current Ask turn without starting another input.
    AskStop: POST "/ask/conversations/{id}/stop", Rule("conversation owner stops own turn"), none, none, json AskReceipt, 200,
        operation Op::facets(&[Facet::required(KeyAction::Write, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) })]);
    /// Keeps temporary Ask or forks a permanent built-in thread as a new Private thread.
    AskKeep: POST "/ask/conversations/{id}/keep", Rule("temporary owner or permanent-thread View and Fork, with destination creation authority"), none, json AskKeepBody, json AskKeepReply, 201,
        operation Op::receipt(&Op::facets(&[
            Facet::required(KeyAction::Read, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) }),
            Facet::required(KeyAction::Write, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::Resolved("ask_keep_destination"))
        ]));
    /// Permanently deletes this personal conversation.
    AskDelete: DELETE "/ask/conversations/{id}", Rule("conversation owner or root org lifecycle administrator"), none, none, none, 204,
        operation Op::facets(&[Facet::required(KeyAction::Manage, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) })]);
    /// Changes the owner's display and bounded retention preferences.
    AskUpdate: PATCH "/ask/conversations/{id}", Rule("owner changes personal preferences within root org bounds"), none, json AskUpdateBody, json AskReceipt, 200,
        operation Op::facets(&[Facet::required(KeyAction::Manage, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) })]);
    /// Exports the complete conversation as its owner or an audited root org administrator.
    AskExport: GET "/ask/conversations/{id}/export", Rule("same current owner or explicit root org inspection authority as AskGet"), none, none, raw, 200,
        operation Op::facets(&[Facet::required(KeyAction::Read, &[KeyTargetKind::Object(ObjectKind::Thread)], Target::TypedPath { parameter: "id", kind: KeyTargetKind::Object(ObjectKind::Thread) })]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{Delegation, for_route};
    use crate::routes::Endpoint;

    #[test]
    fn ask_routes_declare_actions_and_use_shared_surfaces() {
        for route in ROUTES {
            let operation = for_route(route).unwrap();
            assert_eq!(operation.delegation, Delegation::Business);
            assert!(crate::surface::HttpSurface::Browser.serves(route));
            assert!(crate::surface::HttpSurface::Key.serves(route));
        }
    }

    #[test]
    fn asking_requires_read_of_the_conversation_used_as_model_input() {
        let operation = for_route(&AskMessageCreate::ROUTE).unwrap();
        let facets = &operation.variants[0].facets;
        assert!(facets.iter().any(|facet| facet.action == KeyAction::Write));
        assert!(facets.iter().any(|facet| facet.action == KeyAction::Read
            && matches!(
                facet.target,
                Target::TypedPath {
                    parameter: "id",
                    ..
                }
            )));
    }

    #[test]
    fn ask_inputs_cannot_rescope_an_existing_conversation() {
        assert!(
            serde_json::from_value::<AskMessageBody>(serde_json::json!({
                "client_id": "again", "content": [], "root_org_id": Id::nil()
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AskKeepBody>(serde_json::json!({
                "root_org_id": Id::nil()
            }))
            .is_err()
        );
    }

    #[test]
    fn input_model_can_keep_choose_or_clear_its_preference() {
        let body = serde_json::json!({"client_id":"choice","content":[]});
        let kept: AskMessageBody = serde_json::from_value(body.clone()).unwrap();
        assert!(kept.model.is_none());
        let mut clear = body.clone();
        clear["model"] = Json::Null;
        let clear: AskMessageBody = serde_json::from_value(clear).unwrap();
        assert_eq!(clear.model, Some(None));
        let mut choice = body;
        choice["model"] = serde_json::json!({"provider_id":Id::nil(),"model":"selected"});
        let choice: AskMessageBody = serde_json::from_value(choice).unwrap();
        assert_eq!(choice.model.unwrap().unwrap().model, "selected");
        assert!(serde_json::to_value(kept).unwrap().get("model").is_none());
        assert!(serde_json::to_value(clear).unwrap()["model"].is_null());
    }

    #[test]
    fn ask_events_have_a_stable_discriminator_and_sequence() {
        let event = AskEvent {
            conversation_id: Id::nil(),
            seq: 7,
            data: AskEventData::Reset,
        };
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(wire["data"]["kind"], "reset");
        assert_eq!(wire["seq"], 7);
        assert_eq!(serde_json::from_value::<AskEvent>(wire).unwrap(), event);
    }
}
