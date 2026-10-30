//! Audit action catalogue, with the actions that are lines of a thing's history and the mark of an override.
//!
//! Whatever needed a permission and changed something writes one audit row,
//! in the transaction that makes the change. An [`AuditAction`] names what
//! was done. The catalogue is closed: a writer has no other names.
//!
//! Some rows are also a line of the **history** of the thing they are about,
//! shown to whoever may view that thing: what was done to it as a thing, by
//! whom and when. [`AuditAction::is_history_line`] says which actions are.
//! The rest is kept in the audit trail alone: sign-ins, what was only read,
//! what was asked for, offered or refused before anything changed, what a
//! conversation, a chat or a discussion records by itself, and what is
//! about a person's own account, a machine or the system.
//!
//! One row can carry one mark more: that an admin put themself on a page
//! that did not give them what they were about to do
//! ([`AuditAction::takes_override_mark`]).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::roles::{UnknownName, words};

words! {
    /// The part of the catalogue an action belongs to.
    pub enum AuditArea ("area of the audit catalogue") {
        /// Signing in and out, and what a person signs in with.
        Authentication = "authentication",
        /// Users, memberships and service accounts.
        Users = "users",
        /// Invitations.
        Invitations = "invitations",
        /// Orgs, projects, topics and threads as nodes.
        Nodes = "nodes",
        /// Roles, pages, groups and requests for access.
        Access = "access",
        /// Settings and secrets.
        Settings = "settings",
        /// What a thread runs on, and what was done with its turns.
        Threads = "threads",
        /// The life of an environment, its capabilities and its shells.
        Environments = "environments",
        /// Files and processes of a workspace.
        Workspace = "workspace",
        /// Providers, profiles, templates, MCP servers, hosts and mounts.
        Resources = "resources",
        /// Machines.
        Machines = "machines",
        /// Documents.
        Documents = "documents",
        /// Announcements, destinations, channels, chats and what agents were
        /// given and did with it.
        Collaboration = "collaboration",
        /// Usage caps and limits.
        Usage = "usage",
        /// The system itself.
        System = "system",
        /// A chat exported, and the archives of a place.
        Exports = "exports",
    }
}

words! {
    /// Who did something that was audited.
    pub enum ActorKind ("kind of actor") {
        /// A person, or a service account.
        User = "user",
        /// Orch itself.
        System = "system",
        /// The agent of a thread.
        Agent = "agent",
    }
}

/// The meaning of an audit act, used by lists and exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    /// Something was made.
    Made,
    /// Something was changed.
    Changed,
    /// Something was removed.
    Deleted,
    /// Who may do what was changed or exceptional access was used.
    Access,
    /// Work was carried out.
    Work,
    /// A sign-in or its proof was used or changed.
    SignIns,
}

macro_rules! audit_actions {
    ( $( $area:ident { $( $variant:ident = $key:literal, $kind:ident, $what:literal; )* } )* ) => {
        /// What was done, as an audit row names it. It is written as its
        /// key, `node.created`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum AuditAction {
            $( $( #[doc = $what] #[serde(rename = $key)] $variant, )* )*
        }

        impl AuditAction {
            /// Every action, in the order of the catalogue.
            pub const ALL: &'static [AuditAction] = &[ $( $( AuditAction::$variant, )* )* ];

            /// Its key, as it is written and stored.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( $( AuditAction::$variant => $key, )* )* }
            }

            /// The part of the catalogue it belongs to.
            #[must_use]
            pub const fn area(self) -> AuditArea {
                match self { $( $( AuditAction::$variant => AuditArea::$area, )* )* }
            }

            /// What it records, in a sentence.
            #[must_use]
            pub const fn records(self) -> &'static str {
                match self { $( $( AuditAction::$variant => $what, )* )* }
            }

            /// Whether a row of it is also a line of the history of the
            /// thing it is about, where it is about one that has a history:
            /// a node, a document, a resource. Such a line is shown to
            /// whoever may view that thing, so its summary names who did
            /// what and holds no secret, no policy value and no content.
            #[must_use]
            pub const fn is_history_line(self) -> bool {
                match self { $( $( AuditAction::$variant => audit_actions!(@history $kind), )* )* }
            }
        }

        impl std::str::FromStr for AuditAction {
            type Err = UnknownName;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $( $key => Ok(AuditAction::$variant), )* )*
                    _ => Err(UnknownName::new("audit action", text)),
                }
            }
        }
    };
    (@history line) => { true };
    (@history trail) => { false };
}

// Each action is marked `line`, for a line of a thing's history, or `trail`,
// for what the audit trail alone keeps.
audit_actions! {
    Authentication {
        AuthLogin = "auth.login", trail, "Somebody signed in.";
        AuthLoginFailed = "auth.login_failed", trail, "A sign-in was tried and failed.";
        AuthLocked = "auth.locked", trail, "Wrong tries at a name filled its count, and one proof was locked for that name.";
        AuthLogout = "auth.logout", trail, "Somebody signed out.";
        AuthAdminModeChanged = "auth.admin_mode_changed", trail, "A browser session changed its explicit As admin switch.";
        AdminOpened = "admin.opened", trail, "An object was opened through this session's native administrative reach.";
        AuthPasswordChanged = "auth.password_changed", trail, "A password was changed.";
        AuthPasswordReset = "auth.password_reset", trail, "A password was set anew by a reset link.";
        AuthEmailChanged = "auth.email_changed", trail, "The address of a user was changed.";
        AuthTotpEnabled = "auth.totp_enabled", trail, "An authenticator was switched on as a second step.";
        AuthRecoveryCodesReplaced = "auth.recovery_codes_replaced", trail, "The authenticator's one-use recovery codes were replaced.";
        AuthTotpDisabled = "auth.totp_disabled", trail, "An authenticator was switched off.";
        AuthEmailCodeEnabled = "auth.email_code_enabled", trail, "A code by mail was switched on as a second step.";
        AuthEmailCodeDisabled = "auth.email_code_disabled", trail, "A code by mail was switched off as a second step.";
        AuthSecondStepCleared = "auth.second_step_cleared", trail, "A second step was switched off by a reset link that a system admin made for it.";
        AuthSessionRevoked = "auth.session_revoked", trail, "A session was ended from elsewhere.";
        AuthOtherSessionsRevoked = "auth.other_sessions_revoked", trail, "All the person's other browser sessions were ended; this browser stayed signed in.";
        AuthTokenCreated = "auth.token_created", trail, "An API token was made.";
        AuthTokenRevoked = "auth.token_revoked", trail, "An API token was revoked.";
        AuthIdentityLinked = "auth.identity_linked", trail, "A sign-in through an identity provider was linked to a user.";
        AuthIdentityUnlinked = "auth.identity_unlinked", trail, "A sign-in through an identity provider was unlinked from a user.";
        AuthTokenRefused = "auth.token_refused", trail, "The act auth token refused was recorded.";
    }
    Users {
        UserRegistered = "user.registered", trail, "A user was registered.";
        UserSuspended = "user.suspended", trail, "A user was suspended altogether.";
        UserResumed = "user.resumed", trail, "The suspension of a user was lifted.";
        UserAdminGranted = "user.admin_granted", trail, "A user was made a system admin.";
        UserAdminRevoked = "user.admin_revoked", trail, "A user stopped being a system admin.";
        MemberSuspended = "member.suspended", line, "Somebody was suspended in an org.";
        MemberResumed = "member.resumed", line, "Somebody's suspension in an org was lifted.";
        MemberRemoved = "member.removed", line, "A membership ended: somebody was removed from a tree of orgs, or left it.";
        MemberMoved = "member.moved", line, "A member's domain was changed to another org of the tree.";
        ServiceAccountCreated = "service_account.created", line, "A service account was made.";
        ServiceAccountDeleted = "service_account.deleted", line, "A service account was deleted.";
        ProfileInspected = "profile.inspected", trail, "The act profile inspected was recorded.";
    }
    Invitations {
        InviteCreated = "invite.created", trail, "An invitation was made.";
        InviteRevoked = "invite.revoked", trail, "An invitation was revoked.";
        InviteAccepted = "invite.accepted", line, "An invitation was accepted.";
        OrgInvitationGiven = "org.invitation_given", trail, "A system admin invited somebody to make one team org.";
        OrgInvitationTakenBack = "org.invitation_taken_back", trail, "A system admin took back an invitation to make a team org that was not used.";
    }
    Nodes {
        OrgLimitsChanged = "org.limits_changed", line, "A system admin said whether a root org takes members, or holds orgs inside it.";
        NodeCreated = "node.created", line, "An org, a project, a topic or a thread was made.";
        NodeUpdated = "node.updated", line, "A node was renamed, or what describes it was changed.";
        NodeArchived = "node.archived", line, "A node was archived.";
        NodeUnarchived = "node.unarchived", line, "A node was brought back from the archive.";
        NodeDeleted = "node.deleted", line, "A node was deleted.";
        NodeRestored = "node.restored", line, "A deleted node was restored.";
        NodeTransferred = "node.transferred", line, "The first owner of a node changed.";
        NodeMoved = "node.moved", line, "A project, or an org that lies in another, was moved to another org.";
        TopicStatusChanged = "topic.status_changed", line, "A topic was marked done, or open again.";
        TagCreated = "tag.created", line, "A tag was defined.";
        TagDeleted = "tag.deleted", line, "A tag was deleted.";
        DeletedInspected = "deleted.inspected", trail, "The act deleted inspected was recorded.";
    }
    Access {
        RoleGiven = "role.given", line, "A role was given in a place.";
        RoleChanged = "role.changed", line, "A role held in a place was changed.";
        RoleRemoved = "role.removed", line, "A role was taken away in a place.";
        PermissionAdded = "permission.added", line, "A holder was named in a row of a page; in the Owners row, an owner was added.";
        PermissionUpdated = "permission.updated", line, "An entry of a page was changed: its options, its end or its note.";
        PermissionRemoved = "permission.removed", line, "A holder was taken out of a row of a page; in the Owners row, an owner was taken out.";
        PermissionRowChanged = "permission.row_changed", line, "A row was made to follow or cut loose, locked or unlocked, or a page was reset.";
        GroupCreated = "group.created", line, "A group was made.";
        GroupUpdated = "group.updated", line, "A group was renamed or described anew.";
        GroupDeleted = "group.deleted", line, "A group was deleted.";
        GroupMemberAdded = "group.member_added", line, "Somebody was put into a group.";
        GroupMemberRemoved = "group.member_removed", line, "Somebody was taken out of a group.";
        AccessRequested = "access.requested", trail, "Access was asked for; a batch of requests is one row.";
        AccessApproved = "access.approved", trail, "A request for access was approved.";
        AccessDenied = "access.denied", trail, "A request for access was denied.";
    }
    Settings {
        SettingsUpdated = "settings.updated", line, "A setting was written at a node.";
        SettingsReset = "settings.reset", line, "A setting was taken back at a node, so that it is inherited again.";
        SettingsSecretSet = "settings.secret_set", line, "A secret was written, recorded by its name and never by its value.";
        SettingsSecretDeleted = "settings.secret_deleted", line, "A secret was deleted.";
    }
    Threads {
        ThreadCreated = "thread.created", line, "A thread was created.";
        ThreadBound = "thread.bound", line, "A thread was put on a provider, a profile and an environment.";
        ThreadSwitched = "thread.switched", line, "What a thread runs on changed: its provider, its profile, its arrangement, its environment or its directory.";
        ThreadForked = "thread.forked", line, "A fork was taken of a thread.";
        ThreadExported = "thread.exported", trail, "A thread was exported.";
        TurnInterrupted = "turn.interrupted", trail, "A running turn was stopped.";
        InputCancelled = "input.cancelled", trail, "An input was cancelled by somebody other than its author.";
        ApprovalDecided = "approval.decided", trail, "An approval was decided.";
        ApprovalRuleAdded = "approval.rule_added", line, "The act approval rule added was recorded.";
        ApprovalRuleRevoked = "approval.rule_revoked", line, "The act approval rule revoked was recorded.";
        InputHeld = "input.held", trail, "The act input held was recorded.";
        InputRecovered = "input.recovered", trail, "The act input recovered was recorded.";
        InputScheduleCannotStart = "input_schedule.cannot_start", line, "The act input schedule cannot start was recorded.";
        InputScheduleCreated = "input_schedule.created", line, "The act input schedule created was recorded.";
        InputScheduleDeleted = "input_schedule.deleted", line, "The act input schedule deleted was recorded.";
        InputSchedulePaused = "input_schedule.paused", line, "The act input schedule paused was recorded.";
        InputScheduleRan = "input_schedule.ran", line, "The act input schedule ran was recorded.";
        InputScheduleResumed = "input_schedule.resumed", line, "The act input schedule resumed was recorded.";
        InputScheduleUpdated = "input_schedule.updated", line, "The act input schedule updated was recorded.";
        NativeActionInvoked = "native_action.invoked", trail, "The act native action invoked was recorded.";
        QueueHeld = "queue.held", line, "The act queue held was recorded.";
        QueueResumed = "queue.resumed", line, "The act queue resumed was recorded.";
        ThreadLinksChanged = "thread.links_changed", line, "The act thread links changed was recorded.";
        ThreadMoved = "thread.moved", line, "The act thread moved was recorded.";
        ThreadInputRun = "thread.input_run", line, "Somebody who may use a thread's model ran a waiting message on their own access.";
        ThreadCallRecoveryChanged = "thread.call_recovery_changed", line, "A driver changed automatic call recovery.";
        ThreadGoalSet = "thread.goal_set", line, "A person set a goal on a thread.";
        ThreadGoalChanged = "thread.goal_changed", line, "A person edited, paused or resumed a thread's goal.";
        ThreadGoalCleared = "thread.goal_cleared", line, "A person took a thread's goal away.";
        ThreadGoalEnded = "thread.goal_ended", line, "A thread's goal was met, or stopped without being met.";
    }
    Environments {
        PortsOpened = "ports.opened", line, "A wider audience was admitted to an environment port.";
        PortsUpdated = "ports.updated", line, "A port's program, opening or rule was changed.";
        PortsClosed = "ports.closed", line, "A port opening or rule ended.";
        PortsLinkUsed = "ports.link_used", trail, "A link was used for the first time in a UTC day.";
        EnvironmentProvisioned = "environment.provisioned", line, "The unit of an environment was made.";
        EnvironmentStarted = "environment.started", line, "An environment was started.";
        EnvironmentStopped = "environment.stopped", line, "An environment was stopped: by a person, for being idle, or by a recycle.";
        EnvironmentRecreated = "environment.recreated", line, "The unit of an environment was replaced.";
        EnvironmentRestarted = "environment.restarted", line, "The same unit of an environment was stopped and started with its files kept.";
        EnvironmentUpdateApplied = "environment.update_applied", line, "A pending update of an environment was applied.";
        EnvironmentTemplateChanged = "environment.template_changed", line, "An environment was given another template.";
        EnvironmentLost = "environment.lost", line, "An unreachable environment was declared permanently lost.";
        HostLost = "host.lost", line, "An unreachable host was declared permanently lost.";
        MachineLost = "machine.lost", line, "An unreachable machine was declared permanently lost.";
        EnvironmentMoved = "environment.moved", line, "An environment was moved to another host.";
        EnvironmentRemovalScheduled = "environment.removal_scheduled", line, "The removal of an environment was scheduled.";
        CapabilityRequested = "capability.requested", trail, "An agent asked for a capability.";
        CapabilityGranted = "capability.granted", line, "A capability was given to an environment.";
        CapabilityDenied = "capability.denied", trail, "A request for a capability was denied.";
        CapabilityRevoked = "capability.revoked", line, "A capability was taken back from an environment.";
        TerminalOpened = "terminal.opened", line, "A shell was opened.";
        TerminalClosed = "terminal.closed", line, "A shell was closed.";
        DisplayOpened = "display.opened", line, "A display was opened.";
        BrowserOpened = "browser.opened", line, "A browser was opened.";
        BrowserClosed = "browser.closed", line, "A browser was closed.";
        BrowserProfileCreated = "browser.profile_created", line, "A browser profile was made.";
        BrowserProfileRenamed = "browser.profile_renamed", line, "A browser profile was renamed.";
        BrowserProfileDeleted = "browser.profile_deleted", line, "A browser profile was removed.";
        BrowserDriven = "browser.driven", line, "A person drove a browser.";
        TerminalRecordingRead = "terminal.recording_read", line, "What a shell last printed was read by somebody other than its opener.";
        DisplayClosed = "display.closed", line, "The act display closed was recorded.";
        DisplayDriven = "display.driven", line, "The act display driven was recorded.";
        EnvironmentPurged = "environment.purged", line, "The act environment purged was recorded.";
        EnvironmentRestored = "environment.restored", line, "The act environment restored was recorded.";
        NetworkFailedClosed = "network.failed_closed", line, "The act network failed closed was recorded.";
        NetworkPolicyApplied = "network.policy_applied", line, "The act network policy applied was recorded.";
        UnitHookRan = "unit.hook_ran", line, "A lifecycle hook of the host's operator ran for a unit: the moment, why, how it ended and how long it took, never what it printed.";
        UnitCommandRan = "unit.command_ran", line, "A named command ran after a managed unit started: name, exit, duration and timeout, never argv or output.";
    }
    Workspace {
        WorkspaceFileWritten = "workspace.file_written", line, "A person wrote a file through Orch.";
        WorkspaceFileDeleted = "workspace.file_deleted", line, "A person deleted a file through Orch.";
        WorkspaceFileRenamed = "workspace.file_renamed", line, "A person renamed a file through Orch.";
        WorkspaceFileCopied = "workspace.file_copied", line, "A person copied a file through Orch.";
        WorkspaceChangedByAgent = "workspace.changed_by_agent", line, "A turn of an agent changed files.";
        WorkspaceSnapshotCreated = "workspace.snapshot_created", line, "A snapshot of a workspace was made.";
        ProcessSignalled = "process.signalled", line, "A process was sent a signal.";
        GitCommitted = "git.committed", line, "The act git committed was recorded.";
        GitPushed = "git.pushed", line, "The act git pushed was recorded.";
        WorkspaceSourcePrepared = "workspace.source_prepared", line, "The act workspace source prepared was recorded.";
        WorkspaceStateRecorded = "workspace.state_recorded", line, "The act workspace state recorded was recorded.";
        WorkspaceStateRestored = "workspace.state_restored", line, "The act workspace state restored was recorded.";
        WorkspaceWorktreeCreated = "workspace.worktree_created", line, "The act workspace worktree created was recorded.";
    }
    Resources {
        StorageCleaned = "storage.cleaned", line, "Selected retained storage was cleaned, with each factual outcome.";
        ProviderCreated = "provider.created", line, "A provider was added.";
        ProviderUpdated = "provider.updated", line, "A provider was changed.";
        ProviderSourcesChanged = "provider.sources_changed", line, "The sources of a provider's details were changed.";
        ProviderLogin = "provider.login", line, "A provider was signed in.";
        ProviderLogout = "provider.logout", line, "A provider was signed out.";
        ProviderResetUsed = "provider.reset_used", line, "A confirmed reset of a provider was requested.";
        ProviderWindowStartedOver = "provider.window_started_over", line, "A provider window started over before its scheduled reset.";
        ProviderKeyRotated = "provider.key_rotated", line, "The key of a provider was rotated.";
        ProviderModelsChanged = "provider.models_changed", line, "The models a provider allows were changed.";
        ProviderArrangementsChanged = "provider.arrangements_changed", line, "The arrangements a provider may be used in were changed.";
        ProviderRuntimeRecreated = "provider.runtime_recreated", line, "The runtime of a provider was recreated.";
        ProviderRuntimeUpdated = "provider.runtime_updated", line, "The runtime of a provider was updated.";
        ProviderCredentialIssued = "provider.credential_issued", trail, "A copy of a provider's credential was issued to a machine for a thread.";
        ProviderHandedOver = "provider.handed_over", line, "The credential of a provider was handed over to an environment.";
        ProviderReturned = "provider.returned", line, "A credential that was handed over came back to its provider.";
        ProviderHandoverLost = "provider.handover_lost", line, "A credential that was handed over could not be brought back.";
        ProviderDisabled = "provider.disabled", line, "A provider was disabled.";
        ProviderEnabled = "provider.enabled", line, "A provider was enabled.";
        ProviderDeleted = "provider.deleted", line, "A provider was deleted.";
        AgentCreated = "agent.created", line, "An agent profile was added.";
        AgentUpdated = "agent.updated", line, "An agent profile was changed.";
        AgentDisabled = "agent.disabled", line, "An agent profile was disabled.";
        AgentEnabled = "agent.enabled", line, "An agent profile was enabled.";
        AgentDeleted = "agent.deleted", line, "An agent profile was deleted.";
        EnvironmentCreated = "environment.created", line, "An environment was created or registered.";
        EnvironmentUpdated = "environment.updated", line, "An environment was renamed, configured or given another home.";
        EnvironmentDisabled = "environment.disabled", line, "An environment was disabled.";
        EnvironmentEnabled = "environment.enabled", line, "An environment was enabled.";
        EnvironmentDeleted = "environment.deleted", line, "An environment was removed.";
        TemplateCreated = "template.created", line, "A template was added.";
        TemplateUpdated = "template.updated", line, "A template was changed.";
        TemplateDisabled = "template.disabled", line, "A template was disabled.";
        TemplateEnabled = "template.enabled", line, "A template was enabled.";
        TemplateDeleted = "template.deleted", line, "A template was deleted.";
        McpCreated = "mcp.created", line, "An MCP server was added.";
        McpUpdated = "mcp.updated", line, "An MCP server was changed.";
        McpDisabled = "mcp.disabled", line, "An MCP server was disabled.";
        McpEnabled = "mcp.enabled", line, "An MCP server was enabled.";
        McpDeleted = "mcp.deleted", line, "An MCP server was deleted.";
        McpImported = "mcp.imported", line, "An MCP server was imported from the harness configuration of a machine.";
        McpRestarted = "mcp.restarted", line, "One environment MCP server was restarted with its failure count cleared.";
        SkillCreated = "skill.created", line, "A skill was added.";
        SkillDisabled = "skill.disabled", line, "A skill was disabled.";
        SkillEnabled = "skill.enabled", line, "A skill was enabled.";
        SkillDeleted = "skill.deleted", line, "A skill was deleted.";
        SkillVersionMade = "skill.version_made", line, "A version of a skill was made: imported, updated from where it came from, uploaded or published from its draft.";
        EnvironmentExecChanged = "environment.exec_changed", line, "What an environment's exec node offers was changed, or whether its agents get Orch's own tools.";
        ResourceTransferred = "resource.transferred", line, "The first owner of a resource changed.";
        HostCreated = "host.created", line, "A host was added.";
        HostDeleted = "host.deleted", line, "A host was removed.";
        MountCreated = "mount.created", line, "A mount was added.";
        MountUpdated = "mount.updated", line, "A mount was changed.";
        MountDeleted = "mount.deleted", line, "A mount was deleted.";
        MountLocationAdded = "mount.location_added", line, "A location was added to a mount.";
        MountLocationRemoved = "mount.location_removed", line, "A location was removed from a mount.";
    }
    Machines {
        MachineEnrolled = "machine.enrolled", trail, "A machine was enrolled.";
        MachineRenewed = "machine.renewed", trail, "The certificate of a machine was renewed.";
        MachineRevoked = "machine.revoked", trail, "A machine was revoked.";
        MachineRenamed = "machine.renamed", trail, "A machine was renamed.";
        MachinePolicyChanged = "machine.policy_changed", trail, "A key of a machine's local policy was changed, on the machine or from Orch.";
        MachineProgramsChanged = "machine.programs_changed", trail, "A machine manager registered, tested, removed or reloaded an external program.";
        MachineRolesChanged = "machine.roles_changed", trail, "A machine manager changed the connector roles handed to Orch.";
        MachineUpdateRequested = "machine.update_requested", trail, "A verified physical connector or node update was requested.";
        MachineReportRefused = "machine.report_refused", trail, "Reports from a machine about something it does not hold were refused.";
    }
    Documents {
        DocumentCreated = "document.created", line, "A document was made.";
        DocumentRenamed = "document.renamed", line, "A document was renamed.";
        DocumentLinksTabsChanged = "document.links_tabs_changed", line, "The tabs of a document's Links section were changed.";
        DocumentMoved = "document.moved", line, "A document was moved to another node.";
        DocumentDeleted = "document.deleted", line, "A document was deleted.";
        DocumentRestored = "document.restored", line, "A deleted document was restored.";
        DocumentVersionRestored = "document.version_restored", line, "An earlier version of a document was put back.";
        DocumentTransferred = "document.transferred", line, "The first owner of a document changed.";
        DocumentExported = "document.exported", line, "A document was exported.";
        DocumentCommentDeleted = "document.comment_deleted", trail, "A comment was deleted by somebody other than its author.";
        DocumentReadByAgent = "document.read_by_agent", trail, "An agent read a document.";
        DocumentCreatedByAgent = "document.created_by_agent", line, "An agent made a document.";
        DocumentEditedByAgent = "document.edited_by_agent", line, "An agent changed a document.";
    }
    Collaboration {
        AnnouncementSent = "announcement.sent", trail, "An announcement was sent to threads.";
        PersonNotified = "person.notified", trail, "A person explicitly notified people about a thing.";
        DestinationCreated = "destination.created", line, "A destination of a node was added.";
        DestinationUpdated = "destination.updated", line, "A destination of a node was changed.";
        DestinationDeleted = "destination.deleted", line, "A destination of a node was removed.";
        ChannelCreated = "channel.created", line, "A channel of the system or of a root org was set up.";
        ChannelUpdated = "channel.updated", line, "A channel of the system or of a root org was changed.";
        ChannelDeleted = "channel.deleted", line, "A channel of the system or of a root org was removed.";
        ChatCreated = "chat.created", trail, "A group chat was started.";
        ChatRenamed = "chat.renamed", trail, "A group chat was renamed.";
        ChatMemberAdded = "chat.member_added", trail, "Somebody was added to a group chat.";
        ChatMemberRemoved = "chat.member_removed", trail, "Somebody left a group chat, or was removed from it.";
        ChatMessageDeleted = "chat.message_deleted", trail, "A chat message was deleted by somebody other than its author.";
        ChatInspected = "chat.inspected", trail, "A direct or group chat was opened by somebody who is not in it.";
        ThreadReadByAgent = "thread.read_by_agent", trail, "An agent read the conversation of another thread.";
        ChatReadByAgent = "chat.read_by_agent", trail, "An agent read a chat.";
        ShareSearchedByAgent = "share.searched_by_agent", trail, "An agent searched what was shared with it.";
        ShareSnapshotSent = "share.snapshot_sent", line, "A copy was shared with an agent.";
        ShareSnapshotRevoked = "share.snapshot_revoked", line, "A copy that was shared with an agent was taken back.";
        ShareFileDownloaded = "share.file_downloaded", trail, "An agent fetched a file of what was shared with it.";
        ChatPostedByAgent = "chat.posted_by_agent", trail, "An agent wrote in a chat.";
        ThreadSentByAgent = "thread.sent_by_agent", trail, "An agent sent a message to another thread.";
        ChatAgentPostingAllowed = "chat.agent_posting_allowed", line, "The act chat agent posting allowed was recorded.";
        ChatAgentPostingStopped = "chat.agent_posting_stopped", line, "The act chat agent posting stopped was recorded.";
    }
    Usage {
        UsageCapCreated = "usage_cap.created", line, "A usage cap was added.";
        UsageCapUpdated = "usage_cap.updated", line, "A usage cap was changed.";
        UsageCapDeleted = "usage_cap.deleted", line, "A usage cap was removed.";
        UsageCapReached = "usage_cap.reached", trail, "A usage cap was reached.";
        UsageLimitBypassed = "usage.limit_bypassed", line, "Somebody let a turn run past a limit that was reached.";
        RateCatalogueLoaded = "rate_catalogue.loaded", trail, "A rate catalogue was loaded.";
    }
    System {
        SystemSettingsUpdated = "system.settings_updated", trail, "A setting of the system was written.";
        CapabilityDefUpdated = "capability_def.updated", trail, "The definition of a capability was changed.";
        HostUpdated = "host.updated", line, "A host was changed: its name, its labels, or what it does when it is full.";
        IdentityProviderCreated = "identity_provider.created", trail, "An identity provider was added.";
        IdentityProviderUpdated = "identity_provider.updated", trail, "An identity provider was changed.";
        IdentityProviderDeleted = "identity_provider.deleted", trail, "An identity provider was removed.";
        AuditExported = "audit.exported", trail, "The audit trail was exported.";
        HostPortsChanged = "host.ports_changed", line, "The act host ports changed was recorded.";
    }
    Exports {
        ChatExported = "chat.exported", trail, "A chat was exported.";
        ExportCreated = "export.created", trail, "An archive of what a person may read in a place was asked for, and again when it was written, with how much it holds.";
        ExportDownloaded = "export.downloaded", trail, "An archive was fetched for the first time.";
    }
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AuditAction {
    /// The exact kind assigned to this act.
    #[must_use]
    pub const fn kind(self) -> AuditKind {
        match self {
            Self::AgentCreated
            | Self::BrowserProfileCreated
            | Self::ChannelCreated
            | Self::ChatCreated
            | Self::DestinationCreated
            | Self::DocumentCreated
            | Self::DocumentCreatedByAgent
            | Self::EnvironmentCreated
            | Self::ExportCreated
            | Self::HostCreated
            | Self::IdentityProviderCreated
            | Self::InputScheduleCreated
            | Self::McpCreated
            | Self::SkillCreated
            | Self::MountCreated
            | Self::NodeCreated
            | Self::ProviderCreated
            | Self::TagCreated
            | Self::TemplateCreated
            | Self::ThreadCreated
            | Self::ThreadForked
            | Self::ThreadGoalSet
            | Self::UsageCapCreated
            | Self::UserRegistered
            | Self::WorkspaceFileCopied
            | Self::WorkspaceSnapshotCreated
            | Self::WorkspaceWorktreeCreated => AuditKind::Made,
            Self::HostPortsChanged
            | Self::AgentDisabled
            | Self::AgentEnabled
            | Self::AgentUpdated
            | Self::BrowserProfileRenamed
            | Self::CapabilityDefUpdated
            | Self::ChannelUpdated
            | Self::ChatRenamed
            | Self::ChatAgentPostingStopped
            | Self::ChatAgentPostingAllowed
            | Self::DestinationUpdated
            | Self::DocumentEditedByAgent
            | Self::DocumentLinksTabsChanged
            | Self::DocumentMoved
            | Self::DocumentRenamed
            | Self::DocumentRestored
            | Self::DocumentVersionRestored
            | Self::EnvironmentDisabled
            | Self::EnvironmentEnabled
            | Self::EnvironmentExecChanged
            | Self::EnvironmentLost
            | Self::HostLost
            | Self::MachineLost
            | Self::EnvironmentMoved
            | Self::EnvironmentRestored
            | Self::EnvironmentTemplateChanged
            | Self::EnvironmentUpdated
            | Self::HostUpdated
            | Self::IdentityProviderUpdated
            | Self::InputScheduleUpdated
            | Self::MachineRenamed
            | Self::MachineUpdateRequested
            | Self::McpDisabled
            | Self::McpEnabled
            | Self::McpUpdated
            | Self::SkillDisabled
            | Self::SkillEnabled
            | Self::SkillVersionMade
            | Self::MountLocationAdded
            | Self::MountLocationRemoved
            | Self::MountUpdated
            | Self::NodeArchived
            | Self::NodeMoved
            | Self::NodeRestored
            | Self::NodeUnarchived
            | Self::NodeUpdated
            | Self::OrgLimitsChanged
            | Self::ProviderDisabled
            | Self::ProviderEnabled
            | Self::ProviderModelsChanged
            | Self::ProviderUpdated
            | Self::ProviderSourcesChanged
            | Self::SettingsReset
            | Self::SettingsUpdated
            | Self::SystemSettingsUpdated
            | Self::TemplateDisabled
            | Self::TemplateEnabled
            | Self::TemplateUpdated
            | Self::ThreadLinksChanged
            | Self::ThreadMoved
            | Self::ThreadGoalChanged
            | Self::TopicStatusChanged
            | Self::UsageCapUpdated
            | Self::WorkspaceChangedByAgent
            | Self::WorkspaceFileWritten => AuditKind::Changed,
            Self::AgentDeleted
            | Self::ThreadGoalCleared
            | Self::BrowserProfileDeleted
            | Self::ChannelDeleted
            | Self::ChatMessageDeleted
            | Self::DestinationDeleted
            | Self::DocumentCommentDeleted
            | Self::DocumentDeleted
            | Self::EnvironmentDeleted
            | Self::EnvironmentPurged
            | Self::HostDeleted
            | Self::IdentityProviderDeleted
            | Self::InputScheduleDeleted
            | Self::McpDeleted
            | Self::SkillDeleted
            | Self::MountDeleted
            | Self::NodeDeleted
            | Self::ProviderDeleted
            | Self::TagDeleted
            | Self::TemplateDeleted
            | Self::UsageCapDeleted
            | Self::WorkspaceFileDeleted
            | Self::StorageCleaned => AuditKind::Deleted,
            Self::AccessApproved
            | Self::AccessDenied
            | Self::AccessRequested
            | Self::ApprovalDecided
            | Self::ApprovalRuleAdded
            | Self::ApprovalRuleRevoked
            | Self::CapabilityDenied
            | Self::CapabilityGranted
            | Self::CapabilityRequested
            | Self::CapabilityRevoked
            | Self::ChatInspected
            | Self::AdminOpened
            | Self::ChatMemberAdded
            | Self::ChatMemberRemoved
            | Self::ChatReadByAgent
            | Self::DeletedInspected
            | Self::DocumentReadByAgent
            | Self::DocumentTransferred
            | Self::GroupCreated
            | Self::GroupDeleted
            | Self::GroupMemberAdded
            | Self::GroupMemberRemoved
            | Self::GroupUpdated
            | Self::InviteAccepted
            | Self::InviteCreated
            | Self::InviteRevoked
            | Self::MachineEnrolled
            | Self::MachinePolicyChanged
            | Self::MachineProgramsChanged
            | Self::MachineRolesChanged
            | Self::MachineRenewed
            | Self::MachineRevoked
            | Self::MachineReportRefused
            | Self::MemberMoved
            | Self::MemberRemoved
            | Self::MemberResumed
            | Self::MemberSuspended
            | Self::NodeTransferred
            | Self::OrgInvitationGiven
            | Self::OrgInvitationTakenBack
            | Self::PermissionAdded
            | Self::PermissionRemoved
            | Self::PermissionRowChanged
            | Self::PermissionUpdated
            | Self::ProfileInspected
            | Self::ProviderArrangementsChanged
            | Self::ProviderCredentialIssued
            | Self::ProviderHandedOver
            | Self::ProviderHandoverLost
            | Self::ProviderKeyRotated
            | Self::ProviderReturned
            | Self::ResourceTransferred
            | Self::RoleChanged
            | Self::RoleGiven
            | Self::RoleRemoved
            | Self::ServiceAccountCreated
            | Self::ServiceAccountDeleted
            | Self::SettingsSecretDeleted
            | Self::SettingsSecretSet
            | Self::ShareSnapshotRevoked
            | Self::ShareSnapshotSent
            | Self::TerminalRecordingRead
            | Self::ThreadReadByAgent
            | Self::UserAdminGranted
            | Self::UserAdminRevoked
            | Self::UserResumed
            | Self::UserSuspended => AuditKind::Access,
            Self::ProviderResetUsed
            | Self::ProviderWindowStartedOver
            | Self::PortsOpened
            | Self::PortsUpdated
            | Self::PortsClosed
            | Self::PortsLinkUsed
            | Self::AnnouncementSent
            | Self::PersonNotified
            | Self::AuditExported
            | Self::BrowserClosed
            | Self::BrowserDriven
            | Self::BrowserOpened
            | Self::ChatExported
            | Self::ChatPostedByAgent
            | Self::DisplayClosed
            | Self::DisplayDriven
            | Self::DisplayOpened
            | Self::DocumentExported
            | Self::EnvironmentProvisioned
            | Self::EnvironmentRecreated
            | Self::EnvironmentRestarted
            | Self::EnvironmentRemovalScheduled
            | Self::EnvironmentStarted
            | Self::EnvironmentStopped
            | Self::EnvironmentUpdateApplied
            | Self::ExportDownloaded
            | Self::GitCommitted
            | Self::GitPushed
            | Self::InputCancelled
            | Self::InputHeld
            | Self::InputRecovered
            | Self::InputScheduleCannotStart
            | Self::InputSchedulePaused
            | Self::InputScheduleRan
            | Self::InputScheduleResumed
            | Self::McpImported
            | Self::McpRestarted
            | Self::NativeActionInvoked
            | Self::NetworkFailedClosed
            | Self::NetworkPolicyApplied
            | Self::UnitHookRan
            | Self::UnitCommandRan
            | Self::ProcessSignalled
            | Self::ProviderRuntimeRecreated
            | Self::ProviderRuntimeUpdated
            | Self::QueueHeld
            | Self::QueueResumed
            | Self::RateCatalogueLoaded
            | Self::ShareFileDownloaded
            | Self::ShareSearchedByAgent
            | Self::TerminalClosed
            | Self::TerminalOpened
            | Self::ThreadBound
            | Self::ThreadExported
            | Self::ThreadSentByAgent
            | Self::ThreadSwitched
            | Self::TurnInterrupted
            | Self::UsageLimitBypassed
            | Self::ThreadInputRun
            | Self::ThreadGoalEnded
            | Self::ThreadCallRecoveryChanged
            | Self::UsageCapReached
            | Self::WorkspaceFileRenamed
            | Self::WorkspaceSourcePrepared
            | Self::WorkspaceStateRecorded
            | Self::WorkspaceStateRestored => AuditKind::Work,
            Self::AuthEmailChanged
            | Self::AuthEmailCodeDisabled
            | Self::AuthEmailCodeEnabled
            | Self::AuthIdentityLinked
            | Self::AuthIdentityUnlinked
            | Self::AuthLocked
            | Self::AuthLogin
            | Self::AuthLoginFailed
            | Self::AuthLogout
            | Self::AuthAdminModeChanged
            | Self::AuthPasswordChanged
            | Self::AuthPasswordReset
            | Self::AuthSecondStepCleared
            | Self::AuthSessionRevoked
            | Self::AuthOtherSessionsRevoked
            | Self::AuthTokenCreated
            | Self::AuthTokenRevoked
            | Self::AuthTokenRefused
            | Self::AuthTotpDisabled
            | Self::AuthTotpEnabled
            | Self::AuthRecoveryCodesReplaced
            | Self::ProviderLogin
            | Self::ProviderLogout => AuditKind::SignIns,
        }
    }

    /// The first word of its key: `node` of `node.created`.
    #[must_use]
    pub fn noun(self) -> &'static str {
        let key = self.as_str();
        key.split_once('.').map_or(key, |(noun, _)| noun)
    }

    /// The kind of object its rows are about: the first word of its key.
    /// `None` for the actions whose rows are about something else, which the
    /// writer then names by its own kind: the org somebody was suspended in,
    /// the root org a member was moved in, the place a role is held in, the
    /// resource whose first owner changed, and the file an agent fetched.
    #[must_use]
    pub fn object_kind(self) -> Option<&'static str> {
        match self {
            Self::McpRestarted => Some("environment"),
            Self::MemberSuspended
            | Self::MemberResumed
            | Self::MemberMoved
            | Self::RoleGiven
            | Self::RoleChanged
            | Self::RoleRemoved
            | Self::PersonNotified
            | Self::ResourceTransferred
            | Self::ShareFileDownloaded
            | Self::StorageCleaned => None,
            // An invitation to make an org is about the person it was given to: no org exists yet.
            Self::OrgInvitationGiven | Self::OrgInvitationTakenBack => Some("user"),
            _ => Some(self.noun()),
        }
    }

    /// The kind of actor its rows always have, where the catalogue says:
    /// what an agent did, and what Orch found by itself. For the rest the
    /// actor is whoever did it.
    #[must_use]
    pub const fn actor(self) -> Option<ActorKind> {
        match self {
            Self::WorkspaceChangedByAgent
            | Self::DocumentReadByAgent
            | Self::DocumentCreatedByAgent
            | Self::DocumentEditedByAgent
            | Self::ThreadReadByAgent
            | Self::ChatReadByAgent
            | Self::ShareSearchedByAgent
            | Self::ShareFileDownloaded
            | Self::ChatPostedByAgent
            | Self::ThreadSentByAgent => Some(ActorKind::Agent),
            Self::UsageCapReached | Self::ThreadGoalEnded => Some(ActorKind::System),
            _ => None,
        }
    }

    /// Whether a row of it can carry the mark of an override: that an admin of
    /// the place put themself on a page that did not give them what they
    /// were about to do. It is set on the row that names them, and on no
    /// other; the row of the act itself follows it, unmarked.
    #[must_use]
    pub const fn takes_override_mark(self) -> bool {
        matches!(self, Self::PermissionAdded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AuditAction as A;

    #[test]
    fn an_action_is_written_as_its_key() {
        assert_eq!(
            serde_json::to_value(A::ThreadCreated).unwrap(),
            "thread.created"
        );
        assert_eq!(
            "thread.created".parse::<AuditAction>().unwrap(),
            A::ThreadCreated
        );
        assert_eq!(A::ThreadCreated.kind(), AuditKind::Made);
        assert!(A::ThreadCreated.is_history_line());
        assert_eq!(
            serde_json::to_value(A::NodeCreated).unwrap(),
            "node.created"
        );
        assert_eq!(
            serde_json::from_value::<AuditAction>("usage_cap.reached".into()).unwrap(),
            A::UsageCapReached
        );
        assert!(serde_json::from_value::<AuditAction>("node.exploded".into()).is_err());
        let mut seen = std::collections::HashSet::new();
        for action in A::ALL {
            assert!(seen.insert(action.as_str()), "{action} twice");
            assert_eq!(action.as_str().parse::<AuditAction>().unwrap(), *action);
            assert_eq!(action.as_str().split('.').count(), 2, "{action}");
        }
    }

    #[test]
    fn what_was_done_to_a_thing_is_a_line_of_its_history() {
        for line in [
            A::NodeCreated,
            A::NodeUpdated,
            A::SettingsUpdated,
            A::ThreadSwitched,
            A::ThreadForked,
            A::PermissionAdded,
            A::PermissionRowChanged,
            A::NodeTransferred,
            A::RoleGiven,
            A::UsageLimitBypassed,
            A::NodeArchived,
            A::NodeRestored,
            A::MemberMoved,
            A::ProviderLogin,
            A::EnvironmentMoved,
            A::CapabilityGranted,
            A::DocumentVersionRestored,
            A::ShareSnapshotSent,
        ] {
            assert!(line.is_history_line(), "{line}");
        }
    }

    #[test]
    fn what_happened_in_an_environment_is_in_its_history() {
        for line in [
            A::TerminalOpened,
            A::TerminalClosed,
            A::WorkspaceFileWritten,
            A::WorkspaceChangedByAgent,
            A::WorkspaceSnapshotCreated,
            A::ThreadBound,
            A::EnvironmentExecChanged,
            A::McpImported,
        ] {
            assert!(line.is_history_line(), "{line}");
        }
        assert!(
            A::TerminalRecordingRead.is_history_line(),
            "that somebody else read a shell's output is a line"
        );
    }

    #[test]
    fn an_archive_and_the_export_of_a_chat_are_kept_in_the_audit_trail_alone() {
        for action in [A::ChatExported, A::ExportCreated, A::ExportDownloaded] {
            assert!(!action.is_history_line(), "{action}");
            assert_eq!(action.area(), AuditArea::Exports, "{action}");
        }
    }

    #[test]
    fn sign_ins_reads_and_refusals_are_in_no_history() {
        for action in A::ALL
            .iter()
            .filter(|action| action.area() == AuditArea::Authentication)
        {
            assert!(!action.is_history_line(), "{action}");
        }
        for read in [
            A::ThreadExported,
            A::ThreadReadByAgent,
            A::ChatReadByAgent,
            A::DocumentReadByAgent,
            A::ShareSearchedByAgent,
            A::ShareFileDownloaded,
            A::ChatInspected,
            A::AuditExported,
        ] {
            assert!(!read.is_history_line(), "{read}");
        }
        for asked in [
            A::AccessRequested,
            A::AccessDenied,
            A::CapabilityRequested,
            A::CapabilityDenied,
            A::InviteCreated,
            A::InviteRevoked,
        ] {
            assert!(!asked.is_history_line(), "{asked}");
        }
        assert!(
            A::InviteAccepted.is_history_line(),
            "somebody joining is a line of the org's history"
        );
        assert!(
            A::DocumentExported.is_history_line(),
            "an export of a document stands in its history"
        );
    }

    #[test]
    fn what_a_conversation_or_a_chat_records_by_itself_leaves_no_second_line() {
        for said in [
            A::TurnInterrupted,
            A::InputCancelled,
            A::ApprovalDecided,
            A::AnnouncementSent,
            A::ChatMessageDeleted,
            A::ChatPostedByAgent,
            A::ThreadSentByAgent,
            A::DocumentCommentDeleted,
        ] {
            assert!(!said.is_history_line(), "{said}");
        }
        assert!(
            A::DocumentEditedByAgent.is_history_line(),
            "every change an agent made to a document is in its history"
        );
    }

    #[test]
    fn what_is_about_an_account_a_machine_or_the_system_is_in_no_history() {
        for action in [
            A::UserSuspended,
            A::UserAdminGranted,
            A::MachineEnrolled,
            A::MachinePolicyChanged,
            A::MachineProgramsChanged,
            A::MachineRolesChanged,
            A::MachineReportRefused,
        ] {
            assert!(!action.is_history_line(), "{action}");
        }
        for action in [
            A::SystemSettingsUpdated,
            A::CapabilityDefUpdated,
            A::IdentityProviderCreated,
            A::UsageCapReached,
        ] {
            assert!(!action.is_history_line(), "{action}");
        }
        assert!(
            A::HostUpdated.is_history_line(),
            "a host is a resource, wherever the catalogue lists it"
        );
    }

    #[test]
    fn the_mark_of_an_override_is_set_on_the_row_that_names_the_admin_and_on_no_other() {
        let marked: Vec<_> = A::ALL
            .iter()
            .filter(|action| action.takes_override_mark())
            .collect();
        assert_eq!(marked, [&A::PermissionAdded]);
        assert!(
            A::PermissionAdded.is_history_line(),
            "the page's history shows that they were added"
        );
    }

    #[test]
    fn an_actions_object_is_what_its_first_word_names_unless_it_is_about_something_else() {
        assert_eq!(A::NodeCreated.object_kind(), Some("node"));
        assert_eq!(A::UsageCapCreated.object_kind(), Some("usage_cap"));
        assert_eq!(A::IdentityProviderDeleted.noun(), "identity_provider");
        assert_eq!(A::McpImported.object_kind(), Some("mcp"));
        for elsewhere in [
            A::MemberSuspended,
            A::MemberMoved,
            A::RoleGiven,
            A::ResourceTransferred,
            A::ShareFileDownloaded,
        ] {
            assert_eq!(elsewhere.object_kind(), None, "{elsewhere}");
        }
        assert_eq!(A::MemberRemoved.object_kind(), Some("member"));
    }

    #[test]
    fn what_an_agent_did_is_recorded_as_the_agents() {
        for action in A::ALL {
            let by_agent =
                action.as_str().ends_with("_by_agent") || *action == A::ShareFileDownloaded;
            assert_eq!(
                action.actor() == Some(ActorKind::Agent),
                by_agent,
                "{action}"
            );
        }
        assert_eq!(A::UsageCapReached.actor(), Some(ActorKind::System));
        assert_eq!(A::NodeCreated.actor(), None);
    }

    #[test]
    fn the_five_acts_on_a_resource_exist_for_every_kind_that_has_them() {
        for kind in ["provider", "agent", "environment", "template", "mcp"] {
            for verb in ["created", "updated", "disabled", "enabled", "deleted"] {
                let action: AuditAction = format!("{kind}.{verb}").parse().unwrap();
                assert_eq!(action.area(), AuditArea::Resources);
                assert!(action.is_history_line(), "{action}");
            }
        }
    }
}
