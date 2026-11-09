//! Immutable administrator messages and their accepted audience, using the ordinary Inbox.
use crate::common::{Page, PageQuery, ReceiptProjection};
use crate::nodes::NodeRef;
use crate::operations::{FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target};
use crate::routes::Access::{Permission, Rule};
use crate::users::UserRef;
use orch_access_contract::key_scope::{
    KeyAction::{Manage, Read},
    KeyIdentityKind, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A new message; its Markdown text is bounded by the resolved message.body_max_chars setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminMessageCreateBody {
    /// The nonempty heading.
    pub title: String,
    /// Nonempty Markdown, retained as authored.
    pub body: String,
    /// Installation messages may also put their title in the existing system banner until this time.
    /// Organization sends leave this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner_until: Option<Time>,
}

/// The message records its original issuer's name rather than acquiring unrelated live profile data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminMessage {
    /// The stable accepted identity.
    pub id: Id,
    /// None denotes the installation; an org reference is the accepted issuer attribution.
    pub org: Option<NodeRef>,
    /// The accepted heading.
    pub title: String,
    /// The accepted Markdown.
    pub body: String,
    /// The issuer attribution recorded with the message.
    pub sender: UserRef,
    /// When the act was accepted.
    pub created_at: Time,
    /// The number of distinct accounts in the accepted audience.
    pub recipient_count: i64,
    /// The ordinary bounded notice job, where delivery has been queued.
    pub delivery_job_id: Option<Id>,
}

/// A sent-list row omits the potentially large Markdown body, read through MessageGet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminMessageSummary {
    /// The accepted message.
    pub id: Id,
    /// Its accepted issuing scope attribution.
    pub org: Option<NodeRef>,
    /// The accepted heading.
    pub title: String,
    /// The accepted sender attribution.
    pub sender: UserRef,
    /// When the message was accepted.
    pub created_at: Time,
    /// The distinct accepted audience count.
    pub recipient_count: i64,
    /// Its durable delivery job.
    pub delivery_job_id: Option<Id>,
}

/// Sent messages and a current preview of the same native audience the send act captures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AdminMessageListResponse {
    /// A bounded newest-first page, stable at equal timestamps by message identifier.
    #[serde(flatten)]
    pub page: Page<AdminMessageSummary>,
    /// Current count for the composer, not a reservation of the future audience.
    pub audience_count: i64,
}

/// The accepted identity and count disclose no previous content or membership roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminMessageReceipt {
    /// Explicitly content-free.
    pub projection: ReceiptProjection,
    /// The accepted message.
    pub message_id: Id,
    /// Its accepted distinct audience count.
    pub recipient_count: i64,
    /// The durable delivery job, when queued.
    pub delivery_job_id: Option<Id>,
}

/// Existing content is returned only under the send operation's current Read facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AdminMessageCreateResponse {
    /// The native message as the current reader may see it.
    Full(Box<AdminMessage>),
    /// A completed act without reading its content.
    Receipt(AdminMessageReceipt),
}

crate::endpoints! {
    /// Sends an administrator message to this organization's members and sub-organizations.
    OrgMessageCreate: POST "/orgs/{id}/messages", Permission("members.manage"), none, json AdminMessageCreateBody, json AdminMessageCreateResponse, 201,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage, &[KeyTargetKind::Object(ObjectKind::Org)], Target::Path("id")),
            Facet::response_content(&[KeyTargetKind::Object(ObjectKind::Org)], Target::Path("id")),
        ]));
    /// Sends an administrator message to every active person in the installation.
    AdminMessageCreate: POST "/admin/messages", Rule("system_admin"), none, json AdminMessageCreateBody, json AdminMessageCreateResponse, 201,
        operation Op::receipt(&Op::facets(&[
            Facet::required(Manage, &[KeyTargetKind::Identity(KeyIdentityKind::System)], Target::System),
            Facet::response_content(&[KeyTargetKind::Identity(KeyIdentityKind::System)], Target::System),
        ]));
    /// Reads a message received by this account, or sent from a scope it may administer.
    MessageGet: GET "/messages/{id}", Rule("message recipient or sender at its scope"), none, none, json AdminMessage, 200,
        operation Op::single(Read, &[KeyTargetKind::Object(ObjectKind::Org), KeyTargetKind::Identity(KeyIdentityKind::System)], Target::Resolved("message_scope"));
    /// Lists messages sent at this organization, newest first, for whoever may send there.
    OrgMessageList: GET "/orgs/{id}/messages", Permission("members.manage"), query PageQuery, none, json AdminMessageListResponse, 200,
        operation Op::single(Read, &[KeyTargetKind::Object(ObjectKind::Org)], Target::Path("id"));
    /// Lists installation messages, newest first, for a current system administrator.
    AdminMessageList: GET "/admin/messages", Rule("system_admin"), query PageQuery, none, json AdminMessageListResponse, 200,
        operation Op::single(Read, &[KeyTargetKind::Identity(KeyIdentityKind::System)], Target::System);
}
