//! Environment port targets, wider openings, admitted uses and live caller facts.
//!
//! A target exists before a program listens. An opening widens its audience, while a use is
//! one currently admitted connection. Observations and caller records are live measurements.

use orch_types::{Id, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::Holder;
use crate::approvals::{ApprovalState, ApproverScope, Risk};
use crate::common::Page;
use crate::nodes::NodeRef;
use crate::resources::{CapabilityDefaultDecision, HostRef, MachineRef};
use crate::routes::Access::{Permission, Public, Rule};
use crate::users::UserRef;

/// The local socket transport.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PortTransport {
    /// Ordered bytes, with independently ended directions.
    Tcp,
    /// Whole datagrams, without delivery guarantees.
    Udp,
}

/// How a caller reaches an admitted target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortEntryMethod {
    /// HTTP or its sockets through a browser name.
    Browser,
    /// Raw traffic through the caller's loopback helper.
    Helper,
    /// Raw traffic directly to a declared host listener.
    Host,
}

/// A link's selectable entry method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortLinkMethod {
    /// HTTP through the ports domain.
    Browser,
    /// TCP or UDP through the ordinary installation address.
    Helper,
}

/// Who a wider opening admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortOpeningKind {
    /// Current viewers of one named place.
    Place,
    /// Anybody holding its unexpired proof.
    Link,
    /// Anybody reaching the allocated host port.
    Host,
}

/// The program's HTTP transport inside the environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortHttpScheme {
    /// Plain HTTP, as shipped.
    #[default]
    Http,
    /// Local TLS without a certificate trust check.
    Https,
}

/// Options shared by every opening of one environment port.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortProgram {
    /// The program's HTTP transport, independent of the outside scheme.
    pub http_scheme: PortHttpScheme,
    /// An expected Host, or null to pass the caller's Host.
    pub host_header: Option<String>,
    /// Whether the dedicated TCP receiver expects a PROXY v2 prefix.
    pub proxy_protocol: bool,
}

/// What native socket discovery can currently establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortDiscovery {
    /// Current native measurements are available.
    Native,
    /// This platform cannot establish scoped bindings.
    Unsupported,
    /// The environment cannot currently report.
    Offline,
}

/// What a native binding proves about service use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortBinding {
    /// A TCP socket accepts connections.
    Listening,
    /// UDP is bound; whether it serves requests is unknown.
    BoundUnknown,
}

/// A scoped process attributed to a native socket when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortObservationProcess {
    /// Its current native number.
    pub pid: u32,
    /// When this process started.
    pub start: Time,
    /// Its program name.
    pub name: String,
    /// The thread that started it, when known.
    pub thread_id: Option<Id>,
    /// Its originating tool item, when known.
    pub item_id: Option<Id>,
}

/// One current socket binding, independent of persistent admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortObservation {
    /// The local port number.
    pub port: u16,
    /// Its transport.
    pub transport: PortTransport,
    /// Its native bound address.
    pub local_address: String,
    /// The measured runtime generation.
    pub generation: i64,
    /// The native socket identity and observed process start.
    pub observation_id: String,
    /// When this reader first saw this observation.
    pub first_seen_at: Time,
    /// Whether it listens or is only known to be bound.
    pub binding: PortBinding,
    /// Its scoped program, or null when attribution is unknown.
    pub process: Option<PortObservationProcess>,
    /// Whether this is a protected Orch control endpoint.
    pub internal: bool,
    /// Why it cannot currently be used, if applicable.
    pub unavailable_reason: Option<String>,
}

/// Transient UDP losses, without payload or caller content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortSnapshotUdpDropped {
    /// Datagrams over the size bound.
    pub oversized: i64,
    /// Datagrams dropped because the store was full.
    pub full: i64,
    /// New senders beyond the association bound.
    pub senders: i64,
    /// Failed sends to a local socket.
    pub local_send: i64,
    /// Pending datagrams discarded on link loss.
    pub link_lost: i64,
}

/// A current scoped socket measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortSnapshot {
    /// The environment that was measured.
    pub environment_id: Id,
    /// Its runtime generation.
    pub generation: i64,
    /// When the measurement was made.
    pub as_of: Time,
    /// The latest accepted owned use or traffic, not idle presence.
    pub activity_at: Option<Time>,
    /// Whether this is a supported live observation.
    pub discovery: PortDiscovery,
    /// Its unavailable or unsupported reason.
    pub reason: Option<String>,
    /// Current TCP listeners and bound UDP sockets.
    pub listeners: Vec<PortObservation>,
    /// The environment's transient UDP loss counters.
    pub udp_dropped: PortSnapshotUdpDropped,
}

/// Bounded numeric facts that a Ports viewer may read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortCounts {
    /// Current carried connections.
    pub active: i64,
    /// Retained recently ended rows.
    pub recent: i64,
    /// HTTP requests, or null for a raw transport.
    pub http_requests: Option<i64>,
    /// Bytes toward the program.
    pub bytes_in: i64,
    /// Bytes toward callers.
    pub bytes_out: i64,
    /// Where these counters begin.
    pub tracked_since: Time,
    /// When their owner last reset.
    pub reset_at: Time,
}

/// What caller proof an entry requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortEntryNeeds {
    /// A current person's sign-in.
    SignIn,
    /// This opening's unexpired link proof.
    Link,
    /// No caller identity is established.
    None,
}

/// How the program can establish the caller's facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortIdentityMethod {
    /// Replaced forwarding headers with a verifiable assertion.
    HttpHeaders,
    /// The optional TCP prefix on a dedicated receiver.
    ProxyV2Tcp,
    /// An exact active-peer question to the local endpoint.
    PeerLookup,
}

/// An entry ready to use, without a link secret or an access ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortEntry {
    /// How the caller enters.
    pub method: PortEntryMethod,
    /// The transports this entry carries.
    pub transports: Vec<PortTransport>,
    /// Its public address, if available.
    pub address: Option<String>,
    /// Its helper command, containing no credential.
    pub command: Option<String>,
    /// The required proof.
    pub needs: PortEntryNeeds,
    /// The identity mechanisms this entry actually provides.
    pub identity_methods: Vec<PortIdentityMethod>,
}

/// The availability of an environment port, even before its program starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortTargetState {
    /// A usable owned binding was observed.
    Observed,
    /// Its environment is stopped.
    Stopped,
    /// The native measurement found no usable binding.
    NotListening,
    /// Current availability cannot be established.
    Unavailable,
}

/// One stable environment port and its current observations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortTarget {
    /// The owning environment.
    pub environment_id: Id,
    /// The target port number.
    pub port: u16,
    /// Its stable browser address, or null without a ports domain.
    pub address: Option<String>,
    /// The observation's time, including an unavailable read.
    pub as_of: Time,
    /// Its current observation support.
    pub discovery: PortDiscovery,
    /// Its availability, separate from admission.
    pub state: PortTargetState,
    /// Its safe unavailable reason.
    pub reason: Option<String>,
    /// Entries for a door holder, without making an opening.
    pub own_entries: Vec<PortEntry>,
    /// The target's program options.
    pub program: PortProgram,
    /// Zero before the first program write.
    pub revision: Revision,
    /// Current bindings at this port.
    pub listeners: Vec<PortObservation>,
    /// Its currently effective wider openings.
    pub openings: Vec<PortOpening>,
    /// Current counts, or null without a carrier measurement.
    pub counts: Option<PortCounts>,
}

/// A request to deliberately widen one port's audience.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortOpeningRequest {
    /// The local target port, never an outside destination.
    pub port: u16,
    /// The audience to admit.
    pub kind: PortOpeningKind,
    /// The readable place; a thread producer defaults to its topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place_id: Option<Id>,
    /// A link's readable label, not its proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A nonempty unique subset of currently supported transports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transports: Option<Vec<PortTransport>>,
    /// A link's entry choices; other audiences refuse this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_methods: Option<Vec<PortLinkMethod>>,
    /// Required for a link, within the current lifetime policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Time>,
    /// A fixed host number within the saved declaration and local ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    /// Complete reviewed target options, or absent to follow its current ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<PortProgram>,
    /// The deciding person's bounded explanation.
    pub reason: String,
    /// A person's deliberate replacement after a lost once-delivered link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_of: Option<Id>,
    /// Whether that reviewed replacement also closes its old link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_replaced: Option<bool>,
}

/// Persistent admission and its separately measured availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortOpeningState {
    /// Awaiting a manager's decision.
    Waiting,
    /// Admitted, with its initial host bind still pending.
    Opening,
    /// Admitted and currently served.
    Open,
    /// Admitted while its environment is stopped.
    Stopped,
    /// Admitted while no program serves the target.
    NotListening,
    /// Admitted but currently unavailable.
    Unavailable,
    /// The reviewed exposure was refused.
    Refused,
    /// Admission has ended and cannot be revived by restart.
    Closed,
}

/// The intended recipient of a link's one delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortLinkRecipient {
    /// Deliberately disclosed to the requesting thread and its readers.
    Thread,
    /// Given to its deciding or creating person once.
    Person,
}

/// The current reader's permitted actions on an opening.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpeningCan {
    /// Whether this reader may enter this target.
    pub r#use: bool,
    /// Whether this reader may close this opening.
    pub close: bool,
    /// Whether this reader may change this opening.
    pub change: bool,
}

/// One wider audience of an environment port, without an undisclosed proof.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpening {
    /// Its closeable reference.
    pub id: Id,
    /// Its owning environment.
    pub environment_id: Id,
    /// Its local target.
    pub port: u16,
    /// Who is admitted.
    pub kind: PortOpeningKind,
    /// The named place, or null for another audience.
    pub place: Option<NodeRef>,
    /// A readable label.
    pub label: String,
    /// Its selected transports.
    pub transports: Vec<PortTransport>,
    /// Its enabled ways to enter.
    pub entry_methods: Vec<PortEntryMethod>,
    /// Public entries ready to use.
    pub entries: Vec<PortEntry>,
    /// The requesting person, if any.
    pub requested_by: Option<UserRef>,
    /// The requesting thread, if currently readable and retained.
    pub requested_by_thread: Option<NodeRef>,
    /// Its current deciding person.
    pub allowed_by: Option<UserRef>,
    /// The capability grant, template default or policy that admitted this agent opening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<PortOpeningAdmission>,
    /// When it was first created.
    pub opened_at: Time,
    /// Its lapse, or null for an ordinary place or host opening.
    pub until: Option<Time>,
    /// Its admission and current availability state.
    pub state: PortOpeningState,
    /// The safe state explanation.
    pub reason: Option<String>,
    /// The shared target program options.
    pub program: PortProgram,
    /// The host of a direct opening.
    pub host: Option<HostRef>,
    /// The physical machine publishing it, including directory environments without a Host page.
    pub publisher: Option<MachineRef>,
    /// The allocated or previously preferred host number.
    pub host_port: Option<u16>,
    /// Whether the number was explicitly fixed.
    pub fixed_host_port: bool,
    /// Whether it is still reserved, bound or retiring.
    pub host_allocated: bool,
    /// The current opening revision.
    pub revision: Revision,
    /// The explicit link recipient, without their proof.
    pub link_recipient: Option<PortLinkRecipient>,
    /// Current live numeric facts.
    pub counts: Option<PortCounts>,
    /// Actions permitted to this reader now.
    pub can: PortOpeningCan,
}

/// An intended direct result's once-delivered link addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortLinkDelivery {
    /// Whom the review said would receive it.
    pub recipient: PortLinkRecipient,
    /// The root landing address once, when still deliverable.
    pub url_once: Option<String>,
    /// A ready webhook origin with its consumed query proof.
    pub browser_url_once: Option<String>,
    /// The fragment-form link accepted by the helper on stdin.
    pub helper_link_once: Option<String>,
    /// A readable account of the actual intended recipient.
    pub delivered_to: String,
}

/// The bounded decision reference returned beside an opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortRequestRef {
    /// The common approval identifier.
    pub id: Id,
    /// Its current decision state.
    pub state: ApprovalState,
    /// Whether this reader can currently decide.
    pub can_decide: bool,
    /// Its pending lifetime, distinct from the call wait.
    pub expires_at: Option<Time>,
}

/// The one canonical outcome of a human or agent opening request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpenReply {
    /// The opening as it stands now.
    pub opening: PortOpening,
    /// Its decision, where one was needed.
    pub request: Option<PortRequestRef>,
    /// Only this result's explicitly intended link delivery.
    pub link: Option<PortLinkDelivery>,
}

/// The currently reviewed named audience.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpeningPlanAudience {
    /// The current readable named place.
    pub place: Option<NodeRef>,
    /// The complete readable audience count, or null when unknown.
    pub count: Option<i64>,
    /// The actual reviewed audience in words.
    pub sentence: String,
}

/// The exact reviewed host candidate, without binding or reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpeningPlanCandidate {
    /// Its optional Host page.
    pub host: Option<HostRef>,
    /// Its physical publishing machine.
    pub machine: MachineRef,
    /// The exact proposed number.
    pub host_port: u16,
    /// Whether the requester fixed it.
    pub fixed: bool,
    /// The saved host declaration's revision.
    pub host_revision: Revision,
    /// Its local ceiling's revision.
    pub local_limits_revision: Revision,
}

/// One blocker of a reviewed opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpeningPlanBlocker {
    /// A machine-readable reason.
    pub code: String,
    /// The fitting repair in words.
    pub message: String,
}

/// A stored, expiring review which itself opens nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpeningPlan {
    /// The stored review reference.
    pub preflight_id: Id,
    /// Its fifteen-minute review lifetime.
    pub expires_at: Time,
    /// The stable target and truthful observation.
    pub target: PortTarget,
    /// The target options checked at confirmation.
    pub program_revision: Revision,
    /// The complete reviewed request.
    pub request: PortOpeningRequest,
    /// The named audience or public-access sentence.
    pub audience: PortOpeningPlanAudience,
    /// A host candidate, if needed.
    pub candidate: Option<PortOpeningPlanCandidate>,
    /// The explicit link recipient warning before approval.
    pub disclosure: Option<String>,
    /// Current reasons the reviewed effect cannot be confirmed.
    pub blockers: Vec<PortOpeningPlanBlocker>,
}

/// What admitted an agent's opening without a card, as it stood when the opening was made: the
/// capability of its audience granted to the environment, given by its template, or allowed by the
/// policy of a place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortOpeningAdmission {
    /// The capability's key: `port_place`, `port_link` or `port_host`.
    pub capability: String,
    /// The grant, the template default or the policy.
    pub source: PortAdmissionSource,
    /// The place whose `capability.policy` allows it, where the policy did and the reader may see it;
    /// null for the system and for the definition's own default.
    pub scope: Option<NodeRef>,
    /// Whose decision it was: who gave the grant or wrote the policy, where known.
    pub by: Option<UserRef>,
}

/// How a capability admitted an agent's opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortAdmissionSource {
    /// Granted to the environment.
    Grant,
    /// Given by its template from creation.
    Template,
    /// Allowed by `capability.policy` at the environment's home.
    Policy,
}

/// One audience's capability on an environment, as the Ports view shows it beside the openings: whether
/// the template allows it, whether the environment holds it, what the policy decides, what lets an agent
/// open it without a card now, and what this reader may change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortCapability {
    /// Its key: `port_place`, `port_link` or `port_host`.
    pub key: String,
    /// The audience it lets in.
    pub kind: PortOpeningKind,
    /// What it is called.
    pub label: String,
    /// How much harm granting it could do.
    pub risk: Risk,
    /// Whether the environment's template, and the entry its creator used on it, allow it.
    pub offered: bool,
    /// Whether it was granted to this environment (and can be taken back here).
    pub granted: bool,
    /// Whether the template gives it from creation.
    pub from_template: bool,
    /// What `capability.policy` decides of it at the environment's home: a denied kind is opened by
    /// nobody; an asked one makes a card for an agent; an allowed one opens without one.
    pub decision: CapabilityDefaultDecision,
    /// Whose approvers grant it.
    pub approver_scope: ApproverScope,
    /// What lets an agent open this kind without a card now, with where and by whom; null where an
    /// agent's request makes a card or the kind is denied.
    pub admission: Option<PortOpeningAdmission>,
    /// Whether this reader may grant it now.
    pub can_grant: bool,
    /// Whether this reader may take the grant back now (as every capability is taken back).
    pub can_revoke: bool,
}

/// A current audience gate with its deciding scope and holders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentPortsAllowed {
    /// The audience kind.
    pub kind: PortOpeningKind,
    /// Whether current policy permits it.
    pub allowed: bool,
    /// Why it is excluded, if applicable.
    pub reason: Option<String>,
    /// The deciding place, where readable.
    pub scope: Option<NodeRef>,
    /// Its current deciding holders.
    pub holders: Vec<Holder>,
}

/// An inclusive saved host port range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortRange {
    /// Its first number.
    pub from: u16,
    /// Its last number.
    pub to: u16,
}

/// All addresses in both families, or one literal local IP.
pub type PortListen = String;

/// A saved declaration used only after a reviewed host opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostPorts {
    /// The address given to outside callers.
    pub incoming_address: String,
    /// All addresses or one literal local IP.
    pub listen: PortListen,
    /// Inclusive permitted numbers.
    pub ranges: Vec<PortRange>,
}

/// The ceiling a host's own operator sets in the node's configuration, which comes before every
/// declaration of the host's page and which no page can widen. An absent member sets no ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPortsLimits {
    /// The incoming address the node fixes, when its configuration names one.
    pub incoming_address: Option<String>,
    /// `all`, which permits all or one address, or the one literal local IP it permits.
    pub listen: Option<PortListen>,
    /// The ranges a declaration's ranges must lie in; an empty list permits no host opening.
    pub ranges: Option<Vec<PortRange>>,
    /// Changes whenever the ceiling does, so that a review names the ceiling it was made under.
    pub revision: Revision,
}

/// What a host page's three fields are filled with: a proposal, which opens, reserves and binds
/// nothing until a manager reviews and saves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPortsProposalValues {
    /// The address callers would be given, when one could be found.
    pub incoming_address: Option<String>,
    /// `all` where the ceiling permits it, else the one address it permits.
    pub listen: PortListen,
    /// The lowest free block within what the ceiling permits.
    pub ranges: Vec<PortRange>,
}

/// Where each proposed value comes from, said to the manager who reviews it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPortsProposalSources {
    /// For the incoming address.
    pub incoming_address: String,
    /// For the listen choice.
    pub listen: String,
    /// For the ranges.
    pub ranges: String,
}

/// The node's proposal for a host page, with where it comes from and under which ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPortsProposal {
    /// The proposed values.
    pub values: HostPortsProposalValues,
    /// Where each comes from.
    pub sources: HostPortsProposalSources,
    /// When the node made it, from its bindings then.
    pub as_of: Time,
    /// Why something could not be proposed, such as no free block.
    pub reason: Option<String>,
    /// The ceiling it was made under.
    pub limits: HostPortsLimits,
}

/// The canonical live port list, stable targets and surviving admission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentPorts {
    /// The selected environment.
    pub environment_id: Id,
    /// Its current or truthfully unavailable measurement.
    pub snapshot: PortSnapshot,
    /// The current page of observed or deliberately requested targets.
    pub targets: Vec<PortTarget>,
    /// Wider openings visible to this reader.
    pub openings: Vec<PortOpening>,
    /// Each audience's current policy gate.
    pub allowed: Vec<EnvironmentPortsAllowed>,
    /// Each audience's capability on this environment: its grant, its template default and its policy.
    pub capabilities: Vec<PortCapability>,
    /// The distinct wider-port limit.
    pub limit: i64,
    /// Distinct admitted or pending wider ports in use.
    pub used: i64,
    /// The host's saved declaration, if available.
    pub host_ports: Option<HostPorts>,
    /// Whether any effective wider admission survives.
    pub ports_exposed: bool,
    /// The next whole-row page, if any.
    pub cursor: Option<String>,
    /// Whether this answer is incomplete.
    pub truncated: bool,
}

/// An established caller identity, independent of their socket address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PortCaller {
    /// A currently proved person.
    Person {
        /// Their safe reference.
        user: UserRef,
    },
    /// A caller holding the admitted link.
    Link {
        /// The link opening's identifier.
        id: Id,
        /// Its readable label.
        label: String,
    },
    /// An unproved host caller, known only by their outside socket.
    Address,
}

/// The outside peer as the carrier actually observed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortConnectionOutside {
    /// The observed source IP.
    pub address: String,
    /// The observed source port.
    pub port: u16,
    /// The proved person or link, where any was established.
    pub caller: PortCaller,
}

/// The actual socket peer the program sees before interpreting a prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortConnectionInside {
    /// The actual local source address.
    pub address: String,
    /// The actual local source port.
    pub port: u16,
}

/// One live or recently ended use, with no traffic content or assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortConnection {
    /// The connection reference.
    pub id: Id,
    /// Its wider opening, or null for a door holder's own use.
    pub opening_id: Option<Id>,
    /// Its environment.
    pub environment_id: Id,
    /// Its target listener number.
    pub port: u16,
    /// Its transport.
    pub transport: PortTransport,
    /// The current admitted runtime generation.
    pub generation: i64,
    /// The actual outside peer and established identity.
    pub outside: PortConnectionOutside,
    /// The actual distinct inside peer.
    pub inside: PortConnectionInside,
    /// When it began.
    pub since: Time,
    /// Its latest accepted traffic time.
    pub last_active: Time,
    /// Bytes toward the program.
    pub bytes_in: i64,
    /// Bytes toward the caller.
    pub bytes_out: i64,
    /// HTTP request count, or null for raw traffic.
    pub http_requests: Option<i64>,
    /// Its ending time, or null while active.
    pub ended_at: Option<Time>,
}

/// Bounded totals for one caller, or the explicitly grouped overflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortCallerTotal {
    /// The established caller, or null for Other.
    pub caller: Option<PortCaller>,
    /// The observed caller address, or null for Other.
    pub address: Option<String>,
    /// Current connections.
    pub active: i64,
    /// Connections observed since tracking began.
    pub connections: i64,
    /// Bytes toward the program.
    pub bytes_in: i64,
    /// Bytes toward the caller.
    pub bytes_out: i64,
    /// HTTP requests, where applicable.
    pub http_requests: Option<i64>,
    /// The group's last activity.
    pub last_active: Time,
    /// Whether this row represents evicted caller groups.
    pub other: bool,
}

/// Which carrier owns a current memory measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortConnectionOwner {
    /// The through-Orch carrier.
    Gateway,
    /// The direct host carrier.
    Node,
    /// A projection of both independent bounded stores.
    Mixed,
}

/// A live bounded caller page, projected under the reader's current authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortConnectionList {
    /// When its owner measured it.
    pub as_of: Time,
    /// Who owns the transient record.
    pub owner: PortConnectionOwner,
    /// Numeric facts available to a viewer.
    pub counts: PortCounts,
    /// Caller rows, or null for a viewer without Manage.
    pub connections: Option<Vec<PortConnection>>,
    /// Caller totals, or null for a viewer without Manage.
    pub totals: Option<Vec<PortCallerTotal>>,
    /// The continuation of its whole-row page.
    pub cursor: Option<String>,
    /// Whether retained rows or caller groups were cut.
    pub truncated: bool,
}

/// One short-lived connection admission, never a self opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortUse {
    /// Its admission reference.
    pub id: Id,
    /// Its environment.
    pub environment_id: Id,
    /// Its target.
    pub port: u16,
    /// Its one admitted transport.
    pub transport: PortTransport,
    /// Its wider opening, or null for a door holder.
    pub opening_id: Option<Id>,
    /// The single-use socket credential, absent from general lists.
    pub ticket: String,
    /// The admission ticket's short life.
    pub ticket_expires_at: Time,
}

/// Safe metadata returned only after a link proof is checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortLinkInfo {
    /// Its target port.
    pub port: u16,
    /// Its readable label.
    pub label: String,
    /// Its lapse.
    pub until: Time,
    /// Its truthful admission and availability.
    pub state: PortOpeningState,
    /// Its admitted entry methods.
    pub entry_methods: Vec<PortEntryMethod>,
    /// Its ready public entries.
    pub entries: Vec<PortEntry>,
}

/// An exact active peer record with its fresh target-bound assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortPeer {
    /// The same two-end row the carrier reports.
    #[serde(flatten)]
    pub connection: PortConnection,
    /// A fresh assertion; a known peer without one is unavailable.
    pub assertion: String,
}

/// One public verification key, containing no private key member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortAssertionKey {
    /// The public key type, OKP.
    pub kty: PortAssertionKeyType,
    /// Its curve, Ed25519.
    pub crv: PortAssertionCurve,
    /// Its fully specified algorithm, Ed25519.
    pub alg: PortAssertionAlgorithm,
    /// Its use, sig.
    pub r#use: PortAssertionKeyUse,
    /// Its rotating key reference.
    pub kid: String,
    /// Its base64url public bytes.
    pub x: String,
}

/// The public port assertion key type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PortAssertionKeyType {
    /// An octet key pair's public half.
    #[serde(rename = "OKP")]
    Okp,
}

/// The port assertion key's fixed curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PortAssertionCurve {
    /// The Ed25519 curve.
    Ed25519,
}

/// The fully specified port assertion signature algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PortAssertionAlgorithm {
    /// An Ed25519 signature.
    Ed25519,
}

/// A port assertion key's fixed verification use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PortAssertionKeyUse {
    /// Verifies signatures.
    #[serde(rename = "sig")]
    Sig,
}

/// Public verification context for the exact environment and port audience.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortAssertionKeys {
    /// The installation's public issuer.
    pub issuer: String,
    /// The target-specific template expanded by the verifying program.
    pub audience_template: String,
    /// Public verification keys only.
    pub keys: Vec<PortAssertionKey>,
}

/// One current port approval's reviewed request and disclosure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PortRequest {
    /// The original environment, even if the thread moves later.
    pub environment: crate::environment::EnvironmentRef,
    /// Its reviewed widening request.
    pub request: PortOpeningRequest,
    /// Its pending opening.
    pub opening: PortOpening,
    /// Its current stored review.
    pub plan: PortOpeningPlan,
    /// The explicit intended link recipient.
    pub link_recipient: PortLinkRecipient,
    /// What the person reads before approving disclosure.
    pub recipient_warning: Option<String>,
    /// Whether the card offers "Grant to this environment": the audience's capability can be granted
    /// here and those who decide the card are its approvers.
    pub can_grant: bool,
}

// A port request holds no floating-point member anywhere in it, so its derived equality is an
// equivalence; the approval it is part of compares as a whole.
impl Eq for PortRequest {}

/// What the deciding person chooses for a port request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortDecisionChoice {
    /// Admit this opening only.
    Once,
    /// Admit it and grant the audience's gateway capability to this environment.
    Grant,
    /// Refuse it; the same request is refused again for an hour without another card.
    Deny,
}

/// The body of a decision on a port request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortDecision {
    /// What the person chose.
    pub decision: PortDecisionChoice,
    /// For a link: the requesting thread (the default), or the deciding person alone ("For me
    /// only").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_recipient: Option<PortLinkRecipient>,
    /// The revision of the waiting opening the person reviewed.
    pub expected_revision: Revision,
}

/// The environment port list's filters and whole-row page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPortListQuery {
    /// A single stable target, including an unlisted port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Whether to ask the native reader again now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<bool>,
    /// The ordinary bounded page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The preceding whole-row continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Whether to refresh one stable target's native observation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPortGetQuery {
    /// Whether to ask the reader now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<bool>,
}

/// A complete revision-checked target options change, opening no audience.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPortProgramUpdateBody {
    /// The complete new options.
    pub program: PortProgram,
    /// Zero for an unmaterialized target, otherwise its current revision.
    pub expected_revision: Revision,
}

/// Confirms the exact stored opening review once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPortOpeningCreateBody {
    /// The full-body idempotency reference.
    pub client_id: Id,
    /// The reviewed plan, without a changed candidate.
    pub preflight_id: Id,
}

/// A current revision-checked link lapse change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortOpeningUpdateBody {
    /// A bounded new lapse, never a closed-link revival.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Time>,
    /// The opening's current revision.
    pub expected_revision: Revision,
}

/// The revision fence for closing an opening or a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortRevisionBody {
    /// Its current revision.
    pub expected_revision: Revision,
}

/// Filters for a current live caller measurement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPortConnectionListQuery {
    /// One wider opening, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_id: Option<Id>,
    /// The listener number, not its peer's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The inside peer port the program logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside_port: Option<u16>,
    /// One transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<PortTransport>,
    /// The ordinary bounded page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The preceding whole-row continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The scoped administrator's safe wider-opening filters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodePortOpeningListQuery {
    /// The audience kind to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PortOpeningKind>,
    /// Its admission or availability state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<PortOpeningState>,
    /// The ordinary bounded page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The preceding continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A person's ephemeral admission to one owned environment port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPortUseCreateBody {
    /// The single transport to carry.
    pub transport: PortTransport,
    /// A named-place opening, or absent for a door holder's own use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_id: Option<Id>,
}

/// A link holder's admission to its chosen method and transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortLinkUseCreateBody {
    /// The proof submitted in the body, never a logged path.
    pub link: String,
    /// The single transport to carry.
    pub transport: PortTransport,
    /// The method admitted by that link.
    pub entry_method: PortLinkMethod,
}

/// A proof-checked link inspection that makes no use or audit mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortLinkInspectBody {
    /// The complete fragment-form proof, in the request body.
    pub link: String,
}

/// The single-use ticket presented for a raw socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortUseSocketQuery {
    /// The ticket of the previously admitted use.
    pub ticket: String,
}

/// A signed-in browser entry into this target only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPortEnterBody {
    /// The wider place opening, or absent for own use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_id: Option<Id>,
}

/// The target's short-lived stable-host browser entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentPortEnterReply {
    /// The one stable host's opaque entry URL.
    pub url: String,
    /// When the single-use code expires.
    pub expires_at: Time,
}

crate::endpoints! {
    /// Lists current scoped bindings and stable environment targets.
    EnvironmentPortList: GET "/environments/{id}/ports", Permission("ports.view"), query EnvironmentPortListQuery, none, json EnvironmentPorts, 200;
    /// Reads one stable target before or after a program listens.
    EnvironmentPortGet: GET "/environments/{id}/ports/{port}", Permission("ports.view"), query EnvironmentPortGetQuery, none, json PortTarget, 200;
    /// Changes one target's program options without widening its audience.
    EnvironmentPortProgramUpdate: PATCH "/environments/{id}/ports/{port}/program", Permission("ports.manage"), none, json EnvironmentPortProgramUpdateBody, json PortTarget, 200;
    /// Reviews an audience and current program and host options without effect.
    EnvironmentPortOpeningPreflight: POST "/environments/{id}/ports/{port}/openings/preflight", Rule("Ports Manage and current View of the named place"), none, json PortOpeningRequest, json PortOpeningPlan, 200;
    /// Confirms the exact stored audience review once.
    EnvironmentPortOpeningCreate: POST "/environments/{id}/ports/{port}/openings", Rule("Ports Manage and current View of the named place"), none, json EnvironmentPortOpeningCreateBody, json PortOpenReply, 201;
    /// Requests this thread's own environment port decision or replays its outcome.
    ThreadPortRequestCreate: POST "/threads/{id}/port-requests", Permission("thread.interact"), none, json PortOpeningRequest, json PortOpenReply, 200;
    /// Reads an opening through the current door or its named audience.
    PortOpeningGet: GET "/port-openings/{opening_id}", Rule("Ports View or the current admitted named-place viewer"), none, none, json PortOpening, 200;
    /// Changes a link's bounded lapse without reviving closed admission.
    PortOpeningUpdate: PATCH "/port-openings/{opening_id}", Rule("Ports Manage on the opening's environment"), none, json PortOpeningUpdateBody, json PortOpening, 200;
    /// Closes one opening and its current uses, idempotently.
    PortOpeningDelete: DELETE "/port-openings/{opening_id}", Rule("Ports Manage or the requesting agent thread"), none, json PortRevisionBody, json PortOpening, 200;
    /// Reads live counts and the caller facts current Manage permits.
    EnvironmentPortConnectionList: GET "/environments/{id}/ports/connections", Permission("ports.view"), query EnvironmentPortConnectionListQuery, none, json PortConnectionList, 200;
    /// Lists safe effective wider openings under an administered place.
    NodePortOpeningList: GET "/nodes/{id}/port-openings", Rule("Admin of this place; Close uses ordinary Ports Manage or explicit bypass"), query NodePortOpeningListQuery, none, json Page<PortOpening>, 200;
    /// Grants the environment one audience's port capability, so that agents open that kind without a card; nothing of its unit changes. It is taken back as every capability is.
    EnvironmentPortCapabilityGrant: PUT "/environments/{id}/ports/capabilities/{key}", Rule("Ports Manage, and among the capability's approvers by its policy"), none, none, json PortCapability, 200;
    /// Admits one owned port connection without a self opening.
    EnvironmentPortUseCreate: POST "/environments/{id}/ports/{port}/use", Rule("Current Ports View or View of a named-place opening admitting this port"), none, json EnvironmentPortUseCreateBody, json PortUse, 200;
    /// Admits a valid link's one selected entry and transport.
    PortLinkUseCreate: POST "/port-links/use", Public, none, json PortLinkUseCreateBody, json PortUse, 200;
    /// Carries the admitted use as TCP bytes or whole UDP datagrams.
    PortUseSocket: GET "/ws/ports/{use_id}", Public, query PortUseSocketQuery, none, raw, 101;
    /// Makes a signed-in short-lived entry for the target's stable browser host.
    EnvironmentPortEnter: POST "/environments/{id}/ports/{port}/enter", Rule("Signed-in current door holder or admitted named-place viewer"), none, json EnvironmentPortEnterBody, json EnvironmentPortEnterReply, 200;
    /// Serves a generic link landing and platform downloads without private metadata.
    PortLinkLanding: GET "/ports/links/{opening_id}", Public, none, none, raw, 200;
    /// Reads safe link metadata after proof without making a connection.
    PortLinkInspect: POST "/port-links/inspect", Public, none, json PortLinkInspectBody, json PortLinkInfo, 200;
    /// Publishes the installation's target-bound assertion verification keys.
    PortAssertionKeysGet: GET "/ports/assertion-keys", Public, none, none, json PortAssertionKeys, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_port_request_names_an_audience_without_pinning_a_process_or_generation() {
        let written = json!({ "port": 9000, "kind": "place", "reason": "Let the team try it." });
        let request: PortOpeningRequest = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(request.place_id, None);
        assert_eq!(serde_json::to_value(request).unwrap(), written);
        for member in ["pid", "generation", "environment_id", "listener_identity"] {
            let mut bad = written.clone();
            bad[member] = json!(42);
            assert!(serde_json::from_value::<PortOpeningRequest>(bad).is_err());
        }
    }

    #[test]
    fn a_person_can_review_replacement_but_link_choices_do_not_include_host_entry() {
        let written = json!({ "port": 9000, "kind": "link", "reason": "Replace a lost link.",
            "until": "2026-10-14T00:00:00.000Z", "entry_methods": ["browser", "helper"],
            "replacement_of": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "close_replaced": true });
        let request: PortOpeningRequest = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), written);
        let mut bad = written;
        bad["entry_methods"] = json!(["host"]);
        assert!(serde_json::from_value::<PortOpeningRequest>(bad).is_err());
    }

    #[test]
    fn a_peer_is_the_same_two_end_record_with_its_assertion_only_on_the_peer_surface() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "opening_id": null,
            "environment_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "port": 9000,
            "transport": "udp", "generation": 2,
            "outside": { "address": "192.0.2.1", "port": 41001, "caller": { "kind": "address" } },
            "inside": { "address": "127.0.0.1", "port": 41002 },
            "since": "2026-10-13T00:00:00.000Z", "last_active": "2026-10-13T00:00:01.000Z",
            "bytes_in": 0, "bytes_out": 0, "http_requests": null, "ended_at": null,
            "assertion": "signed-for-this-target" });
        let peer: PortPeer = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(serde_json::to_value(&peer).unwrap(), written);
        assert!(
            serde_json::to_value(peer.connection)
                .unwrap()
                .get("assertion")
                .is_none()
        );
    }

    #[test]
    fn public_port_keys_name_only_the_fully_specified_verification_algorithm() {
        let written = json!({ "issuer": "https://orch.example.com", "audience_template": "orch-port:{environment_id}:{port}",
            "keys": [{ "kty": "OKP", "crv": "Ed25519", "alg": "Ed25519", "use": "sig", "kid": "current", "x": "public" }] });
        let keys: PortAssertionKeys = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(serde_json::to_value(keys).unwrap(), written);
        let mut wrong = written;
        wrong["keys"][0]["alg"] = json!("none");
        assert!(serde_json::from_value::<PortAssertionKeys>(wrong).is_err());
    }
}
