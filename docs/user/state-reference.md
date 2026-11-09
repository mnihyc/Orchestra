---
group: work
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [state, status, phase, reference, 状态, 阶段, 参考]
summary: What each state shown beside a thread, provider or machine means.
---

# State reference

States describe reported facts. Offline can recover; lost means an explicit permanent-loss decision. A failed or unavailable state keeps its reason and the action the screen actually offers. Internal names below help match Details to the correct object; similar names do not share all meanings. See [thread states](thread-states.md) and [error recovery](error-reference.md).

## Entries

### ThreadDisplayState

| Reference | Meaning |
| --- | --- |
| `state:ThreadDisplayState.waiting_approval` | A request or held input waits for a person. |
| `state:ThreadDisplayState.blocked` | The thread cannot continue. |
| `state:ThreadDisplayState.failed` | The last turn failed without a later accepted input. |
| `state:ThreadDisplayState.running` | A turn is starting or running. |
| `state:ThreadDisplayState.queued` | Messages wait to start. |
| `state:ThreadDisplayState.held` | A person held the queue until Resume. |
| `state:ThreadDisplayState.finished` | A completed turn has not been seen by this reader. |
| `state:ThreadDisplayState.idle` | No work or unseen result needs an icon. |
| `state:ThreadDisplayState.archived` | The thread or a place above it is archived. |

### PageState

| Reference | Meaning |
| --- | --- |
| `state:PageState.follows` | Every row follows the defaults. |
| `state:PageState.custom` | Somebody changed a row. |
| `state:PageState.restricted` | Its View row no longer follows the defaults, which other products call private. |

### OutsideWayState

| Reference | Meaning |
| --- | --- |
| `state:OutsideWayState.ready` | It is on and nothing is wrong with it. |
| `state:OutsideWayState.needs_attention` | It failed, lost its target or its channel, and says what would repair it. |

### EnvironmentStatus

| Reference | Meaning |
| --- | --- |
| `state:EnvironmentStatus.none` | A record only, until its first use. |
| `state:EnvironmentStatus.provisioning` | Being made. |
| `state:EnvironmentStatus.running` | Up. |
| `state:EnvironmentStatus.stopped` | Stopped. |
| `state:EnvironmentStatus.recreating` | Being replaced by a new unit. |
| `state:EnvironmentStatus.moving` | Moving to another host. |
| `state:EnvironmentStatus.failed` | Failed. |
| `state:EnvironmentStatus.offline` | Its machine is not reachable. |
| `state:EnvironmentStatus.lost` | Explicitly declared permanently unreachable. |
| `state:EnvironmentStatus.disabled` | Switched off. |
| `state:EnvironmentStatus.removed` | Its unit was removed and owned files follow the recorded removal lifecycle. |

### TopicStatus

| Reference | Meaning |
| --- | --- |
| `state:TopicStatus.open` | Work goes on. |
| `state:TopicStatus.done` | It is finished. |

### BackgroundTaskState

| Reference | Meaning |
| --- | --- |
| `state:BackgroundTaskState.running` | Native work still runs. |
| `state:BackgroundTaskState.completed` | Native work finished successfully. |
| `state:BackgroundTaskState.failed` | Native work ended unsuccessfully. |
| `state:BackgroundTaskState.stopped` | A person explicitly ended the work. |
| `state:BackgroundTaskState.lost` | The owning session disappeared. |

### ThreadState

| Reference | Meaning |
| --- | --- |
| `state:ThreadState.idle` | Waiting for something to be said. |
| `state:ThreadState.starting` | Getting ready to take a turn. |
| `state:ThreadState.running` | A turn is under way. |
| `state:ThreadState.waiting_approval` | A turn waits for an approval. |
| `state:ThreadState.blocked` | It cannot go on. |

### HarnessBuildStatus

| Reference | Meaning |
| --- | --- |
| `state:HarnessBuildStatus.untested` | Nothing checked it yet: it is offered as untested. |
| `state:HarnessBuildStatus.checking` | Its check runs. |
| `state:HarnessBuildStatus.checked` | Its interface and native cases passed, or it is the build this release was made with. |
| `state:HarnessBuildStatus.failed` | Its check found something the adapter uses missing, or could not run; see its check. |

### ProviderStatus

| Reference | Meaning |
| --- | --- |
| `state:ProviderStatus.needs_setup` | It has to be set up. |
| `state:ProviderStatus.ready` | Ready. |
| `state:ProviderStatus.exhausted` | The vendor last reported exhaustion; observational, never a local quota gate. |
| `state:ProviderStatus.error` | Something is wrong. |
| `state:ProviderStatus.offline` | Its machine is not reachable. |
| `state:ProviderStatus.disabled` | Switched off. |

### SourceState

| Reference | Meaning |
| --- | --- |
| `state:SourceState.pending` | Waiting for preparation. |
| `state:SourceState.preparing` | Preparation is running. |
| `state:SourceState.ready` | The directory is ready. |
| `state:SourceState.failed` | Preparation failed, retaining its frozen source and waiting input. |

### JobState

| Reference | Meaning |
| --- | --- |
| `state:JobState.queued` | Waiting for its turn. |
| `state:JobState.running` | Under way. |
| `state:JobState.succeeded` | Finished well. |
| `state:JobState.failed` | Failed. |
| `state:JobState.cancelled` | Stopped by somebody. |
| `state:JobState.lost` | The machine needed for this work was explicitly declared lost. |

### GoalStatus

| Reference | Meaning |
| --- | --- |
| `state:GoalStatus.active` | Orch keeps the thread working on it. |
| `state:GoalStatus.paused` | Held: a person paused it, or the agent said it is blocked. |
| `state:GoalStatus.met` | The condition holds. |
| `state:GoalStatus.stopped` | It ended without the condition: a limit was reached, or nobody may go on. |
| `state:GoalStatus.cleared` | A person took it away. |

### GoalReportStatus

| Reference | Meaning |
| --- | --- |
| `state:GoalReportStatus.working` | It is working on it. |
| `state:GoalReportStatus.met` | The condition holds. |
| `state:GoalReportStatus.blocked` | Only a person can go on. |

### FirstStepsState

| Reference | Meaning |
| --- | --- |
| `state:FirstStepsState.done` | Observed complete. |
| `state:FirstStepsState.open` | Still open. |
| `state:FirstStepsState.unknown` | It could not be observed. |

### PortOpeningState

| Reference | Meaning |
| --- | --- |
| `state:PortOpeningState.waiting` | Awaiting a manager's decision. |
| `state:PortOpeningState.opening` | Admitted, with its initial host bind still pending. |
| `state:PortOpeningState.open` | Admitted and currently served. |
| `state:PortOpeningState.stopped` | Admitted while its environment is stopped. |
| `state:PortOpeningState.not_listening` | Admitted while no program serves the target. |
| `state:PortOpeningState.unavailable` | Admitted but currently unavailable. |
| `state:PortOpeningState.refused` | The reviewed exposure was refused. |
| `state:PortOpeningState.closed` | Admission has ended and cannot be revived by restart. |

### AuditObjectState

| Reference | Meaning |
| --- | --- |
| `state:AuditObjectState.live` | The object is live. |
| `state:AuditObjectState.deleted` | Retained after deletion. |
| `state:AuditObjectState.purged` | Physical purge was confirmed. |

### EnvMountState

| Reference | Meaning |
| --- | --- |
| `state:EnvMountState.attached` | It is attached. |
| `state:EnvMountState.unavailable` | It could not be attached. |
| `state:EnvMountState.pending` | It is not created or attached yet. |

### EnvironmentRemovalPhase

| Reference | Meaning |
| --- | --- |
| `state:EnvironmentRemovalPhase.scheduled` | Waiting for unit removal. |
| `state:EnvironmentRemovalPhase.removing` | Removing the unit. |
| `state:EnvironmentRemovalPhase.kept` | Keeping the owned volumes. |
| `state:EnvironmentRemovalPhase.restoring` | Reserving volumes for restoration. |
| `state:EnvironmentRemovalPhase.purging` | Removing the kept volumes. |
| `state:EnvironmentRemovalPhase.purged` | Physical removal was confirmed. |
| `state:EnvironmentRemovalPhase.lost` | The host was declared lost; deletion was never confirmed. |

### CapState

| Reference | Meaning |
| --- | --- |
| `state:CapState.ok` | There is room. |
| `state:CapState.warning` | The warning percent was passed. |
| `state:CapState.reached` | There is no room. |

### MemberState

| Reference | Meaning |
| --- | --- |
| `state:MemberState.unsupported` | The source does not declare it, and it was never told. |
| `state:MemberState.not_read_yet` | The source declares it, and it was never told. |
| `state:MemberState.empty` | It was told as `null` or `[]`: none offered. |
| `state:MemberState.told` | It was told with a value. |
| `state:MemberState.unsupported_because` | The source needs an admin key to offer this fact. This is an unavailable member, not a failed model call. |

### HarnessStatus

| Reference | Meaning |
| --- | --- |
| `state:HarnessStatus.offline` | Its machine is not reachable. |
| `state:HarnessStatus.starting` | Being started. |
| `state:HarnessStatus.online` | Up. |
| `state:HarnessStatus.error` | It failed. |

### MachineStatus

| Reference | Meaning |
| --- | --- |
| `state:MachineStatus.active` | It is registered. |
| `state:MachineStatus.revoked` | Its registration was withdrawn and its session dropped. |

### McpOfferingState

| Reference | Meaning |
| --- | --- |
| `state:McpOfferingState.found` | A different or unrecorded definition was found, and nobody has selected it. |
| `state:McpOfferingState.pending` | Copying the discovered definition under the connector's own policy. |
| `state:McpOfferingState.ready` | The protected definition is available on that machine. |
| `state:McpOfferingState.failed` | Its local policy or native source refused the copy. |

### AgentShareCurrentState

| Reference | Meaning |
| --- | --- |
| `state:AgentShareCurrentState.active` | All requested area rights count on this exact source object for this recipient. |
| `state:AgentShareCurrentState.reduced` | Some requested area rights count on this exact source object for this recipient. |
| `state:AgentShareCurrentState.inactive` | This contribution supplies no rights in the assessed context. |
| `state:AgentShareCurrentState.unassessed` | A per-object or per-recipient check is needed; this is not an inactive grant. |

### PortTargetState

| Reference | Meaning |
| --- | --- |
| `state:PortTargetState.observed` | A usable owned binding was observed. |
| `state:PortTargetState.stopped` | Its environment is stopped. |
| `state:PortTargetState.not_listening` | The native measurement found no usable binding. |
| `state:PortTargetState.unavailable` | Current availability cannot be established. |

### PlanStepStatus

| Reference | Meaning |
| --- | --- |
| `state:PlanStepStatus.pending` | Not begun. |
| `state:PlanStepStatus.in_progress` | Under way. |
| `state:PlanStepStatus.completed` | Done. |

### ApprovalState

| Reference | Meaning |
| --- | --- |
| `state:ApprovalState.pending` | Waiting for a decision. |
| `state:ApprovalState.approved` | Given. |
| `state:ApprovalState.answered` | Content supplied in answer to the harness's question or form. |
| `state:ApprovalState.denied` | Refused. |
| `state:ApprovalState.cancelled` | Taken back by whoever asked. |
| `state:ApprovalState.expired` | Ran out. |

### ThreadContinuationStatus

| Reference | Meaning |
| --- | --- |
| `state:ThreadContinuationStatus.preparing` | Its exact native source is being prepared. |
| `state:ThreadContinuationStatus.ready` | The native result was verified and the continuation can be selected. |
| `state:ThreadContinuationStatus.failed` | Preparation failed; the former execution/history remain intact. |

### InputState

| Reference | Meaning |
| --- | --- |
| `state:InputState.pending` | Waiting its turn. |
| `state:InputState.delivering` | Being handed to the agent. |
| `state:InputState.delivered` | Handed over. |
| `state:InputState.cancelled` | Cancelled before it was. |
| `state:InputState.failed` | It could not be. |

### AgentMessagePhase

| Reference | Meaning |
| --- | --- |
| `state:AgentMessagePhase.commentary` | Said on the way. |
| `state:AgentMessagePhase.final` | The answer. |

### SubagentState

| Reference | Meaning |
| --- | --- |
| `state:SubagentState.running` | At work. |
| `state:SubagentState.completed` | Done. |
| `state:SubagentState.failed` | Failed. |
| `state:SubagentState.interrupted` | Stopped. |

### ItemStatus

| Reference | Meaning |
| --- | --- |
| `state:ItemStatus.in_progress` | Still being produced. |
| `state:ItemStatus.completed` | Done. |
| `state:ItemStatus.failed` | Failed. |
| `state:ItemStatus.cancelled` | Stopped. |
| `state:ItemStatus.pending` | An approval that waits. |
| `state:ItemStatus.approved` | An approval that was given. |
| `state:ItemStatus.answered` | A harness question or form for which a person supplied content. |
| `state:ItemStatus.denied` | An approval that was refused. |
| `state:ItemStatus.expired` | An approval that ran out. |

### TurnState

| Reference | Meaning |
| --- | --- |
| `state:TurnState.starting` | Getting ready. |
| `state:TurnState.running` | Under way. |
| `state:TurnState.completed` | Done. |
| `state:TurnState.interrupted` | Stopped. |
| `state:TurnState.failed` | Failed. |

### TunnelGatewayState

| Reference | Meaning |
| --- | --- |
| `state:TunnelGatewayState.local` | Local connection only. |
| `state:TunnelGatewayState.pending` | Waiting for the gateway connection. |
| `state:TunnelGatewayState.attached` | Attached to the gateway. |
| `state:TunnelGatewayState.unavailable` | The gateway is unavailable. |
| `state:TunnelGatewayState.unknown` | No usable gateway state has been reported. |

### AdoptHistoryState

| Reference | Meaning |
| --- | --- |
| `state:AdoptHistoryState.read` | Its earlier history was read, or it was empty. |
| `state:AdoptHistoryState.unreadable` | The last read failed and may be retried. |
| `state:AdoptHistoryState.unsupported` | The adapter cannot read sessions; another attempt would do nothing. |
| `state:AdoptHistoryState.offline` | The session's machine is offline. |
| `state:AdoptHistoryState.unavailable` | Its provider, environment or machine can no longer be reached. |

### SideState

| Reference | Meaning |
| --- | --- |
| `state:SideState.open` | Those that are open. |
| `state:SideState.closed` | Those that were closed. |

### BackgroundState

| Reference | Meaning |
| --- | --- |
| `state:BackgroundState.live` | Fresh facts of an existing native session. |
| `state:BackgroundState.offline` | The machine or session is disconnected. |
| `state:BackgroundState.unavailable` | The native facts are unavailable. |

### PhaseState

| Reference | Meaning |
| --- | --- |
| `state:PhaseState.pending` | Not begun. |
| `state:PhaseState.running` | Under way. |
| `state:PhaseState.done` | Done. |
| `state:PhaseState.skipped` | Left out. |

### NativeCatalogState

| Reference | Meaning |
| --- | --- |
| `state:NativeCatalogState.ready` | The session supplied its effective mapping. |
| `state:NativeCatalogState.not_ready` | The session cannot list yet. |
| `state:NativeCatalogState.unsupported` | This adapter cannot guarantee native dispatch. |
| `state:NativeCatalogState.failed` | The actual read failed. |

### SnapshotState

| Reference | Meaning |
| --- | --- |
| `state:SnapshotState.running` | Being taken. |
| `state:SnapshotState.complete` | All of it was taken. |
| `state:SnapshotState.partial` | Some files could not be taken. |
| `state:SnapshotState.failed` | Nothing usable came of it. |
| `state:SnapshotState.unavailable` | Recording could not supply a complete state, with its reason retained. |
| `state:SnapshotState.expired` | The kept state expired; its metadata still explains the turn's fallback. |

### InputScheduleState

| Reference | Meaning |
| --- | --- |
| `state:InputScheduleState.active` | Future occurrences are enabled. |
| `state:InputScheduleState.paused` | Future occurrences are paused. |
| `state:InputScheduleState.finished` | A single occurrence is over. |
| `state:InputScheduleState.deleted` | Future work was removed; queued input is unchanged. |

### InputScheduleRunState

| Reference | Meaning |
| --- | --- |
| `state:InputScheduleRunState.queued` | Ordinary input is queued. |
| `state:InputScheduleRunState.blocked` | Ordinary input is held. |
| `state:InputScheduleRunState.started` | Its ordinary turn started. |
| `state:InputScheduleRunState.failed` | It could not start or was cancelled. |
| `state:InputScheduleRunState.skipped` | A nonexistent civil time or a previous pending occurrence. |

### FetchStatus

| Reference | Meaning |
| --- | --- |
| `state:FetchStatus.ready` | That exact version is in the blob store. |
| `state:FetchStatus.fetching` | The environment was asked to upload it. |
| `state:FetchStatus.unavailable` | It cannot be had. |

### McpState

| Reference | Meaning |
| --- | --- |
| `state:McpState.starting` | Its process and protocol are being started. |
| `state:McpState.running` | Running. |
| `state:McpState.stopped` | Stopped. |
| `state:McpState.failed` | It failed. |
| `state:McpState.unavailable` | It cannot be had there. |

### TerminalListState

| Reference | Meaning |
| --- | --- |
| `state:TerminalListState.open` | Those that run. |
| `state:TerminalListState.closed` | Those that ended. |

### TerminalState

| Reference | Meaning |
| --- | --- |
| `state:TerminalState.opening` | Being started. |
| `state:TerminalState.open` | Running. |
| `state:TerminalState.closed` | Ended. |

### BrowserState

| Reference | Meaning |
| --- | --- |
| `state:BrowserState.opening` | Its place is reserved while its process starts. |
| `state:BrowserState.open` | Its driving address answers. |
| `state:BrowserState.closed` | It has ended. |

### ApprovalListState

| Reference | Meaning |
| --- | --- |
| `state:ApprovalListState.pending` | Waiting for a decision. |
| `state:ApprovalListState.decided` | Decided. |

### ModelInputState

| Reference | Meaning |
| --- | --- |
| `state:ModelInputState.prepared` | Recorded, and being sent. |
| `state:ModelInputState.sent` | The harness took it. |
| `state:ModelInputState.failed` | The harness refused it, or it could not be sent. |
| `state:ModelInputState.uncertain` | Whether it arrived is not known: the answer never came. |

### AdminUserStatus

| Reference | Meaning |
| --- | --- |
| `state:AdminUserStatus.active` | They may. |
| `state:AdminUserStatus.suspended` | Their sign-in is ended everywhere: every session and token of theirs is refused at once, and what they own stays theirs. |

### PluginState

| Reference | Meaning |
| --- | --- |
| `state:PluginState.running` | It runs. |
| `state:PluginState.restarting` | It is being started again. |
| `state:PluginState.failed` | It failed. |

### SemanticState

| Reference | Meaning |
| --- | --- |
| `state:SemanticState.off` | Search by meaning is off. |
| `state:SemanticState.not_named` | It is on, and no embeddings provider and model are named. |
| `state:SemanticState.unavailable` | The database cannot keep vectors. |
| `state:SemanticState.idle` | Every piece has its vector. |
| `state:SemanticState.indexing` | Pieces are being cut or made into vectors. |
| `state:SemanticState.paused` | Indexing waits for what its reason names: the disk share, the provider, its machine, a cap or access. |
| `state:SemanticState.waiting` | A new model waits for an admin's choice: the disk share cannot hold both generations. |

### ProgramChannelState

| Reference | Meaning |
| --- | --- |
| `state:ProgramChannelState.none` | No automatic release source was declared. |
| `state:ProgramChannelState.unread` | A source exists but has not been read successfully. |
| `state:ProgramChannelState.ready` | A successful read has been recorded. |
| `state:ProgramChannelState.failed` | The latest attempt failed. |
| `state:ProgramChannelState.disabled` | Scheduled reads are disabled by the system interval. |

### AgentShareSnapshotState

| Reference | Meaning |
| --- | --- |
| `state:AgentShareSnapshotState.pending` | It waits for the thread's next turn. |
| `state:AgentShareSnapshotState.delivered` | It was written into the thread's directory. |
| `state:AgentShareSnapshotState.revoked` | It was taken back. |

### ExportState

| Reference | Meaning |
| --- | --- |
| `state:ExportState.pending` | A job is writing it. |
| `state:ExportState.ready` | It can be fetched. |
| `state:ExportState.failed` | The job failed, or was cancelled; nothing was kept. |
| `state:ExportState.expired` | It was removed, by its person or when its time ran out; the row stays for a month. |

### ImportState

| Reference | Meaning |
| --- | --- |
| `state:ImportState.pending` | A job is bringing it. |
| `state:ImportState.done` | It was brought, all of it that was asked for; the report says what each object came to. |
| `state:ImportState.failed` | The job failed; what it had made stays, and the report says how far it came. |
| `state:ImportState.cancelled` | The job was cancelled; what it had made stays, and the report says how far it came. |
| `state:ImportState.undoing` | A job is taking back what it made. |
| `state:ImportState.undone` | What it made was taken back. |

### BackgroundEndState

| Reference | Meaning |
| --- | --- |
| `state:BackgroundEndState.ended` | Native control confirmed the end. |
| `state:BackgroundEndState.already_ended` | This exact command had already ended. |
| `state:BackgroundEndState.still_running` | The command is known still to run. |
| `state:BackgroundEndState.unknown` | Its end could not be proved. |

### SummaryState

| Reference | Meaning |
| --- | --- |
| `state:SummaryState.empty` | No paragraph has been produced yet. |
| `state:SummaryState.fresh` | The paragraph matches its captured current source. |
| `state:SummaryState.stale` | The source changed without a queued automatic update. |
| `state:SummaryState.updating` | A real queued or running update exists. |
| `state:SummaryState.failed` | The last attempt failed; previous text remains readable. |
| `state:SummaryState.no_summarizer` | No producer is selected or available. |

### AskState

| Reference | Meaning |
| --- | --- |
| `state:AskState.ready` | Ready to receive another message. |
| `state:AskState.running` | One turn is active; a second input cannot start another simultaneous turn. |
| `state:AskState.closed` | The owner can no longer enter the conversation’s root organization. |
| `state:AskState.kept` | A permanent thread was created from this conversation’s history. |

### UploadInventoryState

| Reference | Meaning |
| --- | --- |
| `state:UploadInventoryState.complete` | Verified immutable bytes are in the catalogue. |
| `state:UploadInventoryState.partial` | The upload is unfinished; this does not claim that a sender is active now. |
| `state:UploadInventoryState.expired` | The upload session’s completion deadline passed. |
