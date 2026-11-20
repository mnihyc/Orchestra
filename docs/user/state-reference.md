---
group: work
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [state, status, phase, reference, 状态, 阶段, 参考]
summary: Look up a status shown on a screen, what it means and what you can do next.
heading_id.thread-in-the-sidebar: threaddisplaystate
heading_id.current-turn-in-a-thread: threadstate
heading_id.turn-history: turnstate
heading_id.stopping-a-turn: stopstate
heading_id.waiting-for-source-files: blockedreason
heading_id.preparing-source-files: sourcestate
heading_id.topic-board: topicstatus
heading_id.plan-steps: planstepstatus
heading_id.continuing-from-another-point: threadcontinuationstatus
heading_id.side-sessions: sidestate
heading_id.working-in-a-side-session: sidereading
heading_id.session-files: nativehistorystate
heading_id.reading-earlier-history: adopthistorystate
heading_id.why-a-session-reopened: sessionreopenreason
heading_id.session-changes-in-the-thread: markerkind
heading_id.warnings-and-file-changes: markerharnesschanged
heading_id.personal-questions: askstate
heading_id.summaries: summarystate
heading_id.automatic-archiving-and-deletion: noderetentionhold
heading_id.sending-a-message: inputstate
heading_id.what-the-program-received: modelinputstate
heading_id.work-shown-in-a-message: itemstatus
heading_id.scheduled-messages: inputschedulestate
heading_id.a-scheduled-occurrence: inputschedulerunstate
heading_id.inbox-notices: noticedecisionstate
heading_id.why-a-notice-was-skipped: noticesuppressionreason
heading_id.delivery-outside-orch: channelacceptancestate
heading_id.missing-diagnostic-details: rawcaptureomissioncause
heading_id.machine-connection: machinestatus
heading_id.agent-program-connection: harnessstatus
heading_id.gateway-connection: tunnelgatewaystate
heading_id.environment-status: environmentstatus
heading_id.removal-and-recovery: environmentremovalphase
heading_id.mounted-storage: envmountstate
heading_id.file-snapshots: snapshotstate
heading_id.opening-a-file-version: fetchstatus
heading_id.waiting-for-a-file: fetchunavailable
heading_id.stored-uploads: uploadinventorystate
heading_id.checking-an-upload: uploadphase
heading_id.finding-workspace-files: environmentindexreason
heading_id.provider-availability: providerstatus
heading_id.model-information: memberstate
heading_id.usage-limits: capstate
heading_id.checking-a-program-build: harnessbuildstatus
heading_id.checking-for-releases: programchannelstate
heading_id.extensions: pluginstate
heading_id.mcp-servers-found-on-a-machine: mcpofferingstate
heading_id.mcp-server-status: mcpstate
heading_id.sub-agents: subagentstate
heading_id.answering-a-request: approvalstate
heading_id.requests-list: approvalliststate
heading_id.when-auto-is-unavailable: approvalchoicereason
heading_id.checking-automatic-approval: autoapprovalstate
heading_id.program-at-a-port: porttargetstate
heading_id.sharing-a-port: portopeningstate
heading_id.opening-a-view-from-outside: outsidewaystate
heading_id.terminal-session: terminalstate
heading_id.browser-session: browserstate
heading_id.goal-progress: goalstatus
heading_id.agent-reports: goalreportstatus
heading_id.a-goal-waiting-for-you: goalreason
heading_id.checking-the-result: goaloutcome
heading_id.who-accepted-completion: goalcheckedby
heading_id.reaching-background-work: backgroundstate
heading_id.background-tasks: backgroundtaskstate
heading_id.ending-a-command: backgroundendstate
heading_id.jobs: jobstate
heading_id.steps-in-a-job: phasestate
heading_id.search-by-meaning: semanticstate
heading_id.retrying-a-search-update: semanticretrynext
heading_id.exports: exportstate
heading_id.imports: importstate
heading_id.organization-setup: firststepsstate
heading_id.page-access: pagestate
heading_id.who-can-use-a-shared-resource: agentsharecurrentstate
heading_id.files-shared-with-a-thread: agentsharesnapshotstate
heading_id.items-in-the-activity-history: auditobjectstate
heading_id.account-access: adminuserstatus
heading_id.sign-in-check: humancheckedgestate
---

# State reference

Find the screen where you saw the status. Select a label to link to its explanation. Names and dates below are examples. See also [thread states](thread-states.md) and [error recovery](error-reference.md).

## Threads and turns

### Thread in the sidebar

| Label on screen | What it means |
| --- | --- |
| [Waiting for approval](#state:ThreadDisplayState.waiting_approval) | A request or a queued message needs a decision. Open Approvals or the thread to answer. |
| [Blocked](#state:ThreadDisplayState.blocked) | Work is waiting on a requirement. Open the thread to see what it needs. |
| [Failed](#state:ThreadDisplayState.failed) | The last turn failed. Read its error before sending another message. |
| [Running](#state:ThreadDisplayState.running) | The agent is starting or working on a turn. Open the thread to follow its progress. |
| [Messages waiting](#state:ThreadDisplayState.queued) | Messages will start in queue order when the agent is free. Open the queue to review them. |
| [Held](#state:ThreadDisplayState.held) | Messages are saved in the queue. Choose Resume when they should run. |
| [Finished](#state:ThreadDisplayState.finished) | A turn finished since you last read the thread. Open it to see the reply. |
| [Idle](#state:ThreadDisplayState.idle) | The agent is free and you are caught up. Send a message to start work. |
| [Archived](#state:ThreadDisplayState.archived) | This thread or its parent is archived. Open it to read earlier work. |

### Current turn in a thread

| Label on screen | What it means |
| --- | --- |
| [Idle](#state:ThreadState.idle) | Send a message when you want the agent to begin. |
| [Starting](#state:ThreadState.starting) | The agent is getting ready. The thread shows which step it is on. |
| [Running](#state:ThreadState.running) | The agent is working. Follow its replies in the thread. |
| [Waiting for approval](#state:ThreadState.waiting_approval) | Work is waiting for someone to approve a request. Open the request to decide. |
| [Blocked](#state:ThreadState.blocked) | The thread shows what prevented the next step. Resolve the named problem to continue. |

### Turn history

| Label on screen | What it means |
| --- | --- |
| [Starting](#state:TurnState.starting) | The program is preparing this turn. Its progress appears in the thread. |
| [Running](#state:TurnState.running) | Watch the thread for the next reply or request. |
| [Done](#state:TurnState.completed) | The turn has finished. Read its reply and review any file changes. |
| [Stopped](#state:TurnState.interrupted) | The turn stopped early. Read what it finished before deciding how to continue. |
| [Failed](#state:TurnState.failed) | The program reported a failure. Open the turn to read the reason. |

### Stopping a turn

| Label on screen | What it means |
| --- | --- |
| [Stopping…](#state:StopState.stopping) | Your stop request was accepted. Wait for the current work to finish stopping. |
| [Stopped](#state:StopState.stopped) | The turn ended or its preparation was cancelled. Its outcome is in the thread. |

### Waiting for source files

| Label on screen | What it means |
| --- | --- |
| [source preparation failed](#state:BlockedReason.source_failed) | Source files could not be prepared. Open Source, read the cause and choose Prepare again when ready. Your message stays queued. |

### Preparing source files

| Label on screen | What it means |
| --- | --- |
| [The source is waiting to be prepared](#state:SourceState.pending) | Your work is waiting for the source files. Open Source to see the next step. |
| [Preparing the source](#state:SourceState.preparing) | The environment is preparing the files. You can follow progress in Source. |
| [The source could not be prepared](#state:SourceState.failed) | Preparation stopped with an error. Read the cause in Source before trying again. |

### Topic board

| Label on screen | What it means |
| --- | --- |
| [Open](#state:TopicStatus.open) | Work in this topic is still open. Open its board to see the threads. |
| [Done](#state:TopicStatus.done) | This topic has been marked done. Reopen it if work needs to continue. |

### Plan steps

| Label on screen | What it means |
| --- | --- |
| [Not started](#state:PlanStepStatus.pending) | The agent plans to do this step later. Follow the current step above it. |
| [In progress](#state:PlanStepStatus.in_progress) | The agent is working on this step. Its updates appear in the turn. |
| [Done](#state:PlanStepStatus.completed) | The agent marked this step complete. Read the turn for the result. |

### Continuing from another point

| Label on screen | What it means |
| --- | --- |
| [Preparing this message version…](#state:ThreadContinuationStatus.preparing) | The program is preparing the selected point in the conversation. Wait for it to become available. |
| [Viewing a saved message version.](#state:ThreadContinuationStatus.ready) | You can select this continuation and carry on from it. |
| [This message version could not be prepared.](#state:ThreadContinuationStatus.failed) | Preparation failed. You can still read the earlier history and use the previous continuation. |

### Side sessions

| Label on screen | What it means |
| --- | --- |
| [The open side threads](#state:SideState.open) | Open the side session to follow its work or send a message. |
| [Review · closed](#state:SideState.closed) | The side session has ended. Its earlier conversation is still available. |

### Working in a side session

| Label on screen | What it means |
| --- | --- |
| [Can act](#state:SideReading.acting) | This side session can use the tools allowed in the main workspace. Review its actions in the side session. |

### Session files

| Label on screen | What it means |
| --- | --- |
| [Session files kept](#state:NativeHistoryState.retained) | The program found the files it uses to resume this session. You can read the conversation in Orch. |
| [Session files removed](#state:NativeHistoryState.removed) | The program confirmed that its session files are gone. You can still read the conversation in Orch. |
| [Not reported](#state:NativeHistoryState.unreported) | The program has not confirmed whether its session files are present. You can still read the conversation. |

### Reading earlier history

| Label on screen | What it means |
| --- | --- |
| [From the session before Orch](#state:AdoptHistoryState.read) | Earlier messages were read. An empty conversation has no earlier messages to show. |
| [The earlier history could not be read.](#state:AdoptHistoryState.unreadable) | Reading the earlier messages failed. Choose Read again to retry. |
| [This agent program cannot read the session's earlier history.](#state:AdoptHistoryState.unsupported) | This program cannot read earlier messages from an existing session. Continue with the history you can see. |
| [The session's machine is offline. Its history can be read when it returns.](#state:AdoptHistoryState.offline) | Reading can resume when the machine reconnects. Return here when it is online. |
| [The session's machine or provider is no longer available.](#state:AdoptHistoryState.unavailable) | The session's machine, environment or provider is unreachable. Check its availability before trying again. |

### Why a session reopened

| Label on screen | What it means |
| --- | --- |
| [The session was opened again: it was not known to be open, because the server or the program started again.](#state:SessionReopenReason.unknown) | After the server or agent program restarted, it opened the session again. Continue in the same thread. |
| [The session was opened again: the instructions changed.](#state:SessionReopenReason.instructions) | This turn uses the updated instructions. You can read them in the thread settings. |
| [The session was opened again: the limits on its sub-agents changed.](#state:SessionReopenReason.limits) | The program reopened the session to apply the new sub-agent limits. Check Parameters for the values in force. |
| [The session was opened again: a setting that only a new session takes changed.](#state:SessionReopenReason.configuration) | The working directory or another startup setting changed. Check the current workspace in the thread. |
| [The session was opened again: the MCP servers it is offered changed.](#state:SessionReopenReason.mcp) | This turn uses the updated set of MCP servers. Check the thread's tools to see what is available. |
| [The session was opened again: the Orch tools it is offered changed.](#state:SessionReopenReason.tools) | The set of Orch tools changed. The reopened session uses the tools available for this turn. |
| [The session was opened again: the skills it is given changed.](#state:SessionReopenReason.skills) | This turn uses the updated skills. Check the skills available to the agent. |
| [The session was opened again: this program reads the directory's files each time a session opens, so it opens one for every turn.](#state:SessionReopenReason.program_reloads) | The program reloads its context when a session opens. This turn will continue with that context. |
| [The session was opened again: the history of this side thread was read from its parent again.](#state:SessionReopenReason.side_history) | The side session needed updated conversation history. Follow the next turn there. |

### Session changes in the thread

| Label on screen | What it means |
| --- | --- |
| [The session was opened again: it was not known to be open, because the server or the program started again.](#state:MarkerKind.session_reopened) | The program reopened the session before this turn. Read the line to see what changed. |

### Warnings and file changes

| Label on screen | What it means |
| --- | --- |
| [Agent: Connection lost](#state:MarkerHarnessChanged.warning) | The program raised a warning during this turn. Read it alongside the reply to decide whether to act. |
| [Agent updated the changed files.](#state:MarkerHarnessChanged.turn_diff) | The program updated the list of file changes for this turn. Open the changes to review the latest result. |
| [Orch could not read Agent's updated diff. Earlier changed files were kept.](#state:MarkerHarnessChanged.turn_diff_unavailable) | The latest file-change report could not be read. The last readable changes are still shown. |

### Personal questions

| Label on screen | What it means |
| --- | --- |
| [Ready](#state:AskState.ready) | You can send your next question. |
| [Answering](#state:AskState.running) | An answer is being prepared. Wait for it before sending another question. |
| [Access ended](#state:AskState.closed) | You no longer have access to this conversation's organization. Ask an owner about access. |
| [Kept as thread](#state:AskState.kept) | This conversation was kept as a thread. Open the thread to continue from its history. |

### Summaries

| Label on screen | What it means |
| --- | --- |
| [No summary yet](#state:SummaryState.empty) | There is no summary to read yet. You can still open the original content. |
| [Fresh](#state:SummaryState.fresh) | The summary reflects the current content. Open the original for details. |
| [Out of date](#state:SummaryState.stale) | The original changed after this summary was written. Read the original for the latest information. |
| [Scheduled](#state:SummaryState.scheduled) | You can read the previous summary while the update waits for its next allowed start time. |
| [Updating](#state:SummaryState.updating) | A summary update is queued or running. The new text appears when it finishes. |
| [Failed](#state:SummaryState.failed) | The last update failed. You can read the previous summary and check the reason. |
| [No summarizer](#state:SummaryState.no_summarizer) | A suitable summarizer is missing or unavailable. Check the summary settings. |

### Automatic titles and tags

| Label on screen | What it means |
| --- | --- |
| [Next update](#state:AutomaticScheduleState.queued) | An update is queued. The time shown is the earliest it can start. |
| [Waiting to retry](#state:AutomaticScheduleState.retrying) | An earlier attempt failed. Check the time shown for the next retry. |
| [Skipped · No summarizer](#state:AutomaticScheduleState.skipped) | The automatic update was skipped. The reason beside the label tells you what prevented it. |

### Automatic archiving and deletion

| Label on screen | What it means |
| --- | --- |
| [Active work](#state:NodeRetentionHold.active_work) | Active work keeps this place open. Automatic archiving can proceed after work finishes. |
| [Already archived](#state:NodeRetentionHold.already_archived) | This place is already archived. You can still open it to read its history. |
| [Parent archived](#state:NodeRetentionHold.ancestor_archived) | A parent is archived. Check the parent's status before changing this place. |
| [Not archived](#state:NodeRetentionHold.not_archived) | Automatic deletion applies after archiving. This place is still open. |

## Messages

### Sending a message

| Label on screen | What it means |
| --- | --- |
| [Messages waiting](#state:InputState.pending) | Your message is in the queue. You can review it before its turn starts. |
| [Sending…](#state:InputState.delivering) | The message is being handed to the agent. Wait for delivery to finish. |
| [Sent](#state:InputState.delivered) | The agent received your message. Follow the turn for its reply. |
| [Cancelled](#state:InputState.cancelled) | The message was cancelled before delivery. Send it again if it is still needed. |
| [Not sent](#state:InputState.failed) | The message could not be delivered. Check the reason in the thread before trying again. |

### What the program received

| Label on screen | What it means |
| --- | --- |
| [Prepared](#state:ModelInputState.prepared) | Your input is ready and delivery is under way. Wait for the program to confirm it. |
| [Sent](#state:ModelInputState.sent) | The program accepted this input. You can see exactly what was sent in the thread. |
| [Failed to send](#state:ModelInputState.failed) | The program refused the input or delivery failed. Read the error before retrying. |
| [Delivery not confirmed](#state:ModelInputState.uncertain) | The program did not confirm delivery. Check the thread for a reply before resending. |

### Work shown in a message

| Label on screen | What it means |
| --- | --- |
| [Running](#state:ItemStatus.in_progress) | The agent is still producing this part of the reply. Updates appear here. |
| [Done](#state:ItemStatus.completed) | This part of the turn is complete. Open it to read the result. |
| [Failed](#state:ItemStatus.failed) | This part of the turn failed. Open it to read the error. |
| [Stopped](#state:ItemStatus.cancelled) | This work was stopped. Any output already shown remains available. |
| [Waiting](#state:ItemStatus.pending) | A request is waiting. Open it to answer or make a decision. |
| [Approved](#state:ItemStatus.approved) | Permission was granted. Follow the turn to see what the agent did next. |
| [Answered](#state:ItemStatus.answered) | Someone answered the program's question. Open the request to see the answer. |
| [Denied](#state:ItemStatus.denied) | Permission was refused. Open the request to see the decision. |
| [Expired](#state:ItemStatus.expired) | The time to decide has passed. The agent needs a new request to try this action again. |

### Scheduled messages

| Label on screen | What it means |
| --- | --- |
| [Active](#state:InputScheduleState.active) | Messages will be sent at the times shown. Open the schedule to see the next one. |
| [Paused](#state:InputScheduleState.paused) | Future sends are paused. Resume the schedule when you want them to continue. |
| [Finished](#state:InputScheduleState.finished) | The one-time send is over. Check its occurrence to see whether a turn started. |
| [Cancelled](#state:InputScheduleState.deleted) | Future sends have been removed. Messages already queued stay in the thread. |

### A scheduled occurrence

| Label on screen | What it means |
| --- | --- |
| [Queued](#state:InputScheduleRunState.queued) | The scheduled message is in the thread's queue. It will start when eligible. |
| [Held](#state:InputScheduleRunState.blocked) | The scheduled message is being held. Open the thread to see what must change. |
| [Started](#state:InputScheduleRunState.started) | A turn started for this message. Open the thread to follow it. |
| [Could not start](#state:InputScheduleRunState.failed) | The message could not start or was cancelled. Read the occurrence details. |
| [Skipped](#state:InputScheduleRunState.skipped) | No message was added for this time. Check whether the local time was skipped or the previous send was still waiting. |

### Inbox notices

| Label on screen | What it means |
| --- | --- |
| [Eligible](#state:NoticeDecisionState.eligible) | This notice qualified for your Inbox. For delivery to another channel, check the channel status. |
| [Suppressed](#state:NoticeDecisionState.suppressed) | Orch skipped this notice. Open its details to see why. |
| [Not evaluated](#state:NoticeDecisionState.not_evaluated) | Orch did not check whether to send this notice. The turn itself is available in the thread. |

### Why a notice was skipped

| Label on screen | What it means |
| --- | --- |
| [Interrupted](#state:NoticeSuppressionReason.interrupted) | The turn was stopped, so Orch skipped the usual end-of-turn notice. Open the thread for its outcome. |
| [Did not start](#state:NoticeSuppressionReason.did_not_start) | Work failed before a turn began. Open the thread for the startup error. |
| [Automatic recovery](#state:NoticeSuppressionReason.automatic_recovery) | Automatic recovery is handling this failure. Follow its progress in the thread. |
| [Already watching](#state:NoticeSuppressionReason.already_watching) | You were already watching when the turn ended. Its result is in the thread. |
| [Subagent notices off](#state:NoticeSuppressionReason.subagent_notices_off) | Sub-agent notices were off when this happened. Change your notification preferences for future turns. |

### Delivery outside Orch

| Label on screen | What it means |
| --- | --- |
| [Pending](#state:ChannelAcceptanceState.pending) | The notification channel has not confirmed delivery yet. Check this status again later. |
| [Accepted by channel](#state:ChannelAcceptanceState.accepted) | The channel accepted the notice. Whether it appeared on your device depends on that channel. |
| [Not confirmed](#state:ChannelAcceptanceState.unknown) | The channel may have received the notice. Check it before requesting another delivery. |

### Missing diagnostic details

| Label on screen | What it means |
| --- | --- |
| [Capture off](#state:RawCaptureOmissionCause.capture_off) | Diagnostic capture was off at the time. Those details cannot be recovered from this turn. |
| [Masking inputs unavailable](#state:RawCaptureOmissionCause.masking_unavailable) | Orch could not safely hide sensitive values, so it left out the message contents. Other available details remain visible. |
| [Capture allowance reached](#state:RawCaptureOmissionCause.capture_allowance) | Newer diagnostic messages took the available space. Only the remaining messages can be inspected. |
| [Retention elapsed](#state:RawCaptureOmissionCause.retention_elapsed) | These details reached the end of their retention period and were removed. The turn can still be read. |

## Machines

### Machine connection

| Label on screen | What it means |
| --- | --- |
| [Online](#state:MachineStatus.active) | The machine is connected. Open it to see the environments available there. |
| [Offline](#state:MachineConnection.offline) | The machine is disconnected. Check its connection or wait for it to come back online. |
| [Lost](#state:MachineConnection.lost) | Someone marked the machine lost after its allowed absence. You can still read the threads and check which environments can be recovered. |
| [Revoked](#state:MachineStatus.revoked) | This machine's registration was revoked. Register it again if it needs to connect. |

### Agent program connection

| Label on screen | What it means |
| --- | --- |
| [Offline](#state:HarnessStatus.offline) | The program's machine is unreachable. Check the machine connection. |
| [Starting](#state:HarnessStatus.starting) | The program is starting. Wait for it to come online. |
| [Online](#state:HarnessStatus.online) | The program is online and can receive work. |
| [Error](#state:HarnessStatus.error) | The program reported an error. Open its details before trying to start work. |

### Gateway connection

| Label on screen | What it means |
| --- | --- |
| [Local](#state:TunnelGatewayState.local) | The tools connect locally. You can use them without a gateway. |
| [Connecting](#state:TunnelGatewayState.pending) | The gateway connection is being established. Wait for it to connect. |
| [Tool connection](#state:TunnelGatewayState.attached) | The named gateway is carrying the thread’s tool requests. You can see which gateway is in use here. |
| [Unavailable](#state:TunnelGatewayState.unavailable) | The gateway cannot be reached. Check its availability before reconnecting. |
| [Not reported](#state:TunnelGatewayState.unknown) | No current gateway status is available. Check its connection details. |

## Environments

### Environment status

| Label on screen | What it means |
| --- | --- |
| [Not started](#state:EnvironmentStatus.none) | The environment has no running workspace yet. Check its setup before starting work. |
| [Being made](#state:EnvironmentStatus.provisioning) | The workspace is being created. Open the environment to follow setup. |
| [Running](#state:EnvironmentStatus.running) | The workspace is running. You can use it for work you have access to. |
| [Stopped](#state:EnvironmentStatus.stopped) | The workspace is stopped. Start it when you need its programs or files. |
| [Being recreated](#state:EnvironmentStatus.recreating) | The workspace is being created again. Follow progress on the environment page. |
| [Being moved](#state:EnvironmentStatus.moving) | The workspace is being moved. Wait for the move to finish before using the new location. |
| [Failed](#state:EnvironmentStatus.failed) | The environment could not complete its last operation. Read the error on its page. |
| [Offline](#state:EnvironmentStatus.offline) | Its machine is disconnected. Check the connection and wait for it to return. |
| [Lost](#state:EnvironmentStatus.lost) | The machine was marked lost. Read the recovery options before choosing a replacement workspace. |
| [Switched off](#state:EnvironmentStatus.disabled) | Use of this environment is disabled. Ask its manager whether it can be enabled. |
| [Removed](#state:EnvironmentStatus.removed) | The environment was removed. Check its removal details to see whether files can be restored. |

### Removal and recovery

| Label on screen | What it means |
| --- | --- |
| [Deletion queued](#state:EnvironmentRemovalPhase.scheduled) | Removal is queued. Check the environment's removal details for progress. |
| [Deleting…](#state:EnvironmentRemovalPhase.removing) | The workspace is being removed. Wait for the result before attempting recovery. |
| [Deleted; files kept](#state:EnvironmentRemovalPhase.kept) | The workspace was removed, but its storage was kept. Review the restore option before the retention period ends. |
| [Restoring…](#state:EnvironmentRemovalPhase.restoring) | The kept storage is being prepared for restoration. Follow the restore progress. |
| [Deleting kept files…](#state:EnvironmentRemovalPhase.purging) | The kept storage is being deleted. Wait for confirmation before considering cleanup complete. |
| [Deleted](#state:EnvironmentRemovalPhase.purged) | Deletion of the storage was confirmed. It can no longer be restored here. |
| [Lost](#state:EnvironmentRemovalPhase.lost) | The machine was marked lost before deletion could be confirmed. Check the machine's recovery details. |

### Mounted storage

| Label on screen | What it means |
| --- | --- |
| [Attached](#state:EnvMountState.attached) | The storage is attached and available in the workspace at the displayed path. |
| [Unavailable](#state:EnvMountState.unavailable) | The storage could not be attached. Read the mount details to find the cause. |
| [Waiting to attach](#state:EnvMountState.pending) | The storage has not been created or attached yet. Check again when setup finishes. |

### File snapshots

| Label on screen | What it means |
| --- | --- |
| [Creating](#state:SnapshotState.running) | Workspace files are being saved. Wait for the snapshot to finish. |
| [Complete](#state:SnapshotState.complete) | All files selected for this snapshot were saved. You can review them before restoring. |
| [Partial](#state:SnapshotState.partial) | Only some files were saved. Check which files are available before restoring. |
| [Failed](#state:SnapshotState.failed) | The snapshot failed. Read the error before relying on it for restoration. |
| [Unavailable](#state:SnapshotState.unavailable) | A complete snapshot is unavailable. Open its details to see the reason and any alternatives. |
| [Expired](#state:SnapshotState.expired) | The saved files have expired. The turn still shows why a different source may be needed. |

### File transfers

| Label on screen | What it means |
| --- | --- |
| [Transferring](#state:FileTransferState.transferring) | The machine has started sending the file. Open its details to see when the latest part arrived. |
| [Interrupted](#state:FileTransferState.interrupted) | The transfer stopped before the whole file arrived. Check the expiry time in its details. |
| [Expired](#state:FileTransferState.expired) | The unfinished transfer expired and its partial data was cleared. Send the file again if you still need it. |

### Opening a file version

| Label on screen | What it means |
| --- | --- |
| [Preview unavailable](#state:FetchStatus.unavailable) | This version cannot be fetched. Check the reason shown beside the file. If the file is relayed through an offline machine, wait for it to reconnect. |

A saved version opens from the file store. When the version is missing, the environment receives a request to upload it.

### Waiting for a file

| Label on screen | What it means |
| --- | --- |
| [Its environment is offline; this file is read directly from that environment.](#state:FetchUnavailable.offline) | The workspace must reconnect before its file can be opened. Check the environment's connection. |

### Stored uploads

| Label on screen | What it means |
| --- | --- |
| [Complete](#state:UploadInventoryState.complete) | The entire upload was verified. You can use the file where you have access. |
| [Partial](#state:UploadInventoryState.partial) | Only part of the file was received. Check the upload before deciding whether to resume. |
| [Expired](#state:UploadInventoryState.expired) | The upload deadline passed before completion. Start a new upload if you still need the file. |

### Checking an upload

| Label on screen | What it means |
| --- | --- |
| [Checking](#state:UploadPhase.verifying) | All pieces arrived and the receiver is checking the file. Wait for confirmation without resending them. |

### Finding workspace files

| Label on screen | What it means |
| --- | --- |
| [the rules could not be read](#state:EnvironmentIndexReason.unreadable) | Some file information or exclusion rules could not be read. Open the file directly to check what is available now. |

## Resource pools

| Label on screen | What it means |
| --- | --- |
| [Choosing](#state:PoolChoiceState.building) | The pool is recording a choice. You can open its history to see the result when it is ready. |
| [Selected](#state:PoolChoiceState.selected) | The pool selected a member. You can see which member it chose if you have access to it and the pools on its path. |
| [No eligible member](#state:PoolChoiceState.exhausted) | No member met the requirements for this attempt. Opening the history shows the recorded result. |

## Providers

### Provider availability

| Label on screen | What it means |
| --- | --- |
| [Needs setup](#state:ProviderStatus.needs_setup) | Setup is incomplete. Open the provider to see which step is missing. |
| [Ready](#state:ProviderStatus.ready) | The provider is available for the models and access it offers. Choose an available model to begin. |
| [Limit reached](#state:ProviderStatus.exhausted) | The provider has reached its allowance. Check its usage window for the next reset. |
| [Error](#state:ProviderStatus.error) | The provider reported a problem. Open its details for the cause and recovery options. |
| [Offline](#state:ProviderStatus.offline) | The provider cannot be reached. Check its machine or connection before trying again. |
| [Disabled](#state:ProviderStatus.disabled) | The provider has been disabled. Ask its manager if it should be enabled. |

### Returning to the original model

| Label on screen | What it means |
| --- | --- |
| [When ready on a later turn](#state:CallReturnState.when_ready) | A later turn may use the original model when it is available. Open the model details to see which one is in use now. |
| [Stay on failover](#state:CallReturnState.stay_on_failover) | The thread will keep using its replacement model. Open the model selector to review your choices. |
| [Returning](#state:CallReturnState.returning) | The switch back to the original model has started. Check the active model in the thread. |
| [Not reported](#state:CallReturnState.unreported) | The thread has no information about a return to the original model. Check the active model before continuing. |

### Trying a provider again

| Label on screen | What it means |
| --- | --- |
| [Reserved](#state:ProviderTrialState.reserved) | One message has been chosen to try this provider. Check the trial details to see which one. |
| [Running](#state:ProviderTrialState.running) | The chosen turn is now trying the provider. Open the thread to follow its progress. |
| [Used](#state:ProviderTrialState.consumed) | The trial has been used. Check the turn’s result to see whether the provider answered. |
| [Not reserved](#state:ProviderTrialState.not_reserved) | This check has no message reserved for a trial. See the provider’s current status before sending work. |

### Model information

| Label on screen | What it means |
| --- | --- |
| [This source does not list models; add them by name.](#state:MemberState.unsupported) | This source does not supply these details. Check what the provider lets you enter yourself. |
| [Models · not read yet](#state:MemberState.not_read_yet) | These details can be read but have not arrived yet. Use the provider's read action if available. |
| [The source lists no models.](#state:MemberState.empty) | The provider returned no entries for this part. Check its account or configuration if you expected some. |
| [Models reported by the source](#state:MemberState.told) | The provider supplied these details. Open the section to read them. |
| [Not offered · needs an admin key](#state:MemberState.unsupported_because) | This detail needs an administrator key. Ask the provider's manager about that access. |

### Usage limits

| Label on screen | What it means |
| --- | --- |
| [Available](#state:CapState.ok) | The work is within its allowance. You can check how much is left in Usage. |
| [Near limit](#state:CapState.warning) | Usage has crossed the warning level. Check the remaining allowance before starting more work. |
| [At limit](#state:CapState.reached) | The allowance is exhausted. Open Usage to see the limit and when room may become available. |

## Agent programs

### Checking a program build

| Label on screen | What it means |
| --- | --- |
| [Untested](#state:HarnessBuildStatus.untested) | This program build has not been checked. Run its check before relying on it. |
| [Checking](#state:HarnessBuildStatus.checking) | The program check is running. Its result will appear here. |
| [Checked](#state:HarnessBuildStatus.checked) | The build passed its check. You can select it where it is available. |
| [Failed](#state:HarnessBuildStatus.failed) | The build failed its check. Read the result before using it. |

### Checking for releases

| Label on screen | What it means |
| --- | --- |
| [No release channel](#state:ProgramChannelState.none) | This program has no automatic release source. Check its installed versions directly. |
| [Not read](#state:ProgramChannelState.unread) | A release source is configured but has not been read successfully. Check its address. |
| [Read](#state:ProgramChannelState.ready) | Release information was read successfully. Open the version list to see what is available. |
| [Read failed](#state:ProgramChannelState.failed) | The last release check failed. Read its reason before trying again. |
| [Disabled](#state:ProgramChannelState.disabled) | Automatic release checks are off. Ask an administrator about the check interval. |

### Extensions

| Label on screen | What it means |
| --- | --- |
| [Running](#state:PluginState.running) | The extension is running. Its features are available where configured. |
| [Restarting](#state:PluginState.restarting) | The extension is starting again. Wait for its status to return to running. |
| [Failed](#state:PluginState.failed) | The extension failed. Open its details to see what needs attention. |

### MCP servers found on a machine

| Label on screen | What it means |
| --- | --- |
| [Found](#state:McpOfferingState.found) | A server definition was found on the machine. Review it before selecting it for use. |
| [Connecting…](#state:McpOfferingState.pending) | The machine is saving the selected definition. Wait for confirmation. |
| [Available](#state:McpOfferingState.ready) | The saved definition can now be used on this machine. |
| [Import failed](#state:McpOfferingState.failed) | The machine could not save the definition. Check the reason and its local settings. |

### MCP server status

| Label on screen | What it means |
| --- | --- |
| [Starting](#state:McpState.starting) | The server is starting. Wait for its tools to become available. |
| [Running](#state:McpState.running) | The server is running. Check which tools it offers this session. |
| [Stopped](#state:McpState.stopped) | The server is stopped. Start it if you need its tools. |
| [Failed](#state:McpState.failed) | The server failed. Read the error before trying to start it again. |
| [Unavailable](#state:McpState.unavailable) | The server cannot be used here. Open its details to see why. |

### Sub-agents

| Label on screen | What it means |
| --- | --- |
| [running](#state:SubagentState.running) | The sub-agent is working. Follow its progress in the parent thread. |
| [done](#state:SubagentState.completed) | The sub-agent finished. Read its result in the parent thread. |
| [failed](#state:SubagentState.failed) | The sub-agent failed. Open its details to read the error. |
| [stopped](#state:SubagentState.interrupted) | The sub-agent was stopped. Check its last output before starting more work. |

## Approvals and questions

### Answering a request

| Label on screen | What it means |
| --- | --- |
| [Waiting](#state:ApprovalState.pending) | Someone needs to decide. Open the request to see what the agent wants to do. |
| [Approved](#state:ApprovalState.approved) | The request was allowed. Follow the thread to see what happened next. |
| [Answered](#state:ApprovalState.answered) | Someone supplied the requested answer. Open the request to read it. |
| [Denied](#state:ApprovalState.denied) | The request was refused. Open it to review the decision. |
| [Cancelled](#state:ApprovalState.cancelled) | The requester withdrew the request. No decision is needed. |
| [Expired](#state:ApprovalState.expired) | The decision deadline passed. Ask for a new request if the action is still needed. |

### Delivering your decision

| Label on screen | What it means |
| --- | --- |
| [Decision accepted · Delivery pending](#state:ApprovalDeliveryState.pending) | Your decision was accepted and is waiting to reach the agent program. Open Details to see the last attempt. |
| [Applied](#state:ApprovalDeliveryState.applied) | The agent program confirmed your decision. Follow the thread to see what it does next. |
| [Decision accepted · Delivery unknown](#state:ApprovalDeliveryState.unknown) | Orch accepted your decision but could not confirm delivery. Open Details for the last attempt and its result. |

### Requests list

| Label on screen | What it means |
| --- | --- |
| [Waiting for me](#state:ApprovalListState.pending) | These requests still need an answer. Choose one to see who can decide. |
| [History](#state:ApprovalListState.decided) | These requests have been decided. Open one to read its outcome. |

### When Auto is unavailable

| Label on screen | What it means |
| --- | --- |
| [Auto approval check has not passed on this machine.](#state:ApprovalChoiceReason.auto_not_proven) | This version has not passed the Auto check on this machine. Choose another available mode or ask for a check. |
| [Auto approval check failed on this machine.](#state:ApprovalChoiceReason.auto_check_failed) | The Auto check failed for this version. Choose another available mode and review the check result. |
| [No Auto check is available for this program.](#state:ApprovalChoiceReason.auto_check_unavailable) | This program has no available Auto check. Choose a supported approval mode. |

### Checking automatic approval

| Label on screen | What it means |
| --- | --- |
| [Auto: Not proven on this machine yet.](#state:AutoApprovalState.not_proven) | The selected build has not passed its Auto check on this machine. A manager can run the check. |
| [Auto: Failed its check on this machine.](#state:AutoApprovalState.failed) | This build failed its Auto check. Read the result or use another available approval mode. |
| [Checked](#state:AutoApprovalState.passed) | The build passed its Auto check. Availability also depends on the reviewer's access and the thread's restrictions. |
| [Auto: No Auto check is available for this program.](#state:AutoApprovalState.unavailable) | This program cannot demonstrate Auto approval here. Choose one of the modes offered. |

## Ports and views

### Program at a port

| Label on screen | What it means |
| --- | --- |
| [Listening](#state:PortTargetState.observed) | A program is listening at this port. Open the view if you have access. |
| [Environment stopped](#state:PortTargetState.stopped) | The environment is stopped. Start it to make the port available again. |
| [Nothing listens](#state:PortTargetState.not_listening) | No program is listening at this port. Start the intended service and check its port number. |
| [Unavailable](#state:PortTargetState.unavailable) | The current port cannot be checked. Look at the environment and its connection. |

### Sharing a port

| Label on screen | What it means |
| --- | --- |
| [Waiting](#state:PortOpeningState.waiting) | A manager is reviewing the request. Check its status before sharing the address. |
| [Opening…](#state:PortOpeningState.opening) | Access was approved and the port is being opened. Wait for it to become available. |
| [Open](#state:PortOpeningState.open) | The port is available at the shown address to the people allowed to use it. |
| [Environment stopped](#state:PortOpeningState.stopped) | Access is approved, but the environment is stopped. Start it to use the port. |
| [Nothing listens](#state:PortOpeningState.not_listening) | Access is approved. Start the program that should listen on this port. |
| [Unavailable](#state:PortOpeningState.unavailable) | Access is approved, but the port cannot be reached. Check the environment's connection. |
| [Refused](#state:PortOpeningState.refused) | The request to share the port was declined. Read the decision before making a new request. |
| [Closed](#state:PortOpeningState.closed) | This opening has ended. A new opening is needed to share the port again. |

### Opening a view from outside

| Label on screen | What it means |
| --- | --- |
| [Ready](#state:OutsideWayState.ready) | The outside address is ready. Use it with the access shown. |
| [Needs attention](#state:OutsideWayState.needs_attention) | This address needs attention before it can be used. Read the connection details. |

### Terminal session

| Label on screen | What it means |
| --- | --- |
| [at its prompt](#state:TerminalState.open) | The terminal is connected. You can enter commands when its prompt is ready. |
| [closed](#state:TerminalState.closed) | This terminal has ended. Start a new terminal if you need a prompt. |

### Browser session

| Label on screen | What it means |
| --- | --- |
| [Starting](#state:BrowserState.opening) | The browser is starting. Wait for the page to appear. |
| [Open](#state:BrowserState.open) | The browser is available. Open its view to interact with the page. |
| [Closed](#state:BrowserState.closed) | This browser session has ended. Start a new one if you need to return. |

## Goals

### Goal progress

| Label on screen | What it means |
| --- | --- |
| [Working](#state:GoalStatus.active) | The agent is working toward the goal. Follow the turns for progress. |
| [Paused](#state:GoalStatus.paused) | Work on the goal is paused. Read the reason before deciding whether to resume. |
| [Met](#state:GoalStatus.met) | The goal was checked or its evidence was accepted by a person. Open the goal to review the result. |
| [Stopped](#state:GoalStatus.stopped) | Work stopped before reaching the goal. Read the reason to see whether it can continue. |
| [Sam cleared the goal](#state:GoalStatus.cleared) | Someone cleared the goal. Set a new goal if more work is needed. |

### Agent reports

| Label on screen | What it means |
| --- | --- |
| [Working](#state:GoalReportStatus.working) | The agent reports that work is continuing. Follow the thread for the next update. |
| [Met](#state:GoalReportStatus.met) | The agent says the condition has been met. Look at its evidence and the check before accepting completion. |
| [Waiting for you](#state:GoalReportStatus.blocked) | The agent needs help to continue. Read its report and address the obstacle. |

### A goal waiting for you

| Label on screen | What it means |
| --- | --- |
| [The completion report needs a person's decision](#state:GoalReason.unchecked) | Completion has not been independently checked. Review the evidence and choose Mark met or Keep working. |

### Checking the result

| Label on screen | What it means |
| --- | --- |
| [Not checked](#state:GoalOutcome.unchecked) | The report and its evidence are available for review. A person can accept completion or ask the agent to continue. |

### Who accepted completion

| Label on screen | What it means |
| --- | --- |
| [confirmed by a person](#state:GoalCheckedBy.person) | A person accepted the completion evidence. Open the goal history to see who made the decision. |

## Background work

### Reaching background work

| Label on screen | What it means |
| --- | --- |
| [Background work](#state:BackgroundState.live) | The session is connected. Open a task to see its current work. |
| [Agent program offline. Its running commands are not known.](#state:BackgroundState.offline) | The session or its machine disconnected. Check again after it reconnects. |
| [Control of the agent program’s commands is unavailable.](#state:BackgroundState.unavailable) | The program cannot report current background work. Check the session for details. |

### Background tasks

| Label on screen | What it means |
| --- | --- |
| [Running](#state:BackgroundTaskState.running) | The task is running. Open it to follow its output. |
| [Finished](#state:BackgroundTaskState.completed) | The task finished. Open its result in the thread. |
| [Failed](#state:BackgroundTaskState.failed) | The task failed. Read its error before starting it again. |
| [Ended](#state:BackgroundTaskState.stopped) | The task was stopped. Check what it completed before it ended. |
| [Lost](#state:BackgroundTaskState.lost) | The program lost the session that was running the task. Check its last output before repeating work. |

### Ending a command

| Label on screen | What it means |
| --- | --- |
| [Ended](#state:BackgroundEndState.ended) | The program confirmed that the command ended. Review its final output if needed. |
| [Already ended](#state:BackgroundEndState.already_ended) | The command had already ended when you asked to stop it. No further stop is needed. |
| [Still running](#state:BackgroundEndState.still_running) | The command is still running. Check its output before deciding whether to stop it again. |
| [End not known](#state:BackgroundEndState.unknown) | The program could not confirm that the command ended. Check its status before starting another copy. |

### Jobs

| Label on screen | What it means |
| --- | --- |
| [Waiting to start](#state:JobState.queued) | The job is waiting for a worker. It will start when one is available. |
| [Under way](#state:JobState.running) | Work is under way. Open the job to follow progress. |
| [Done](#state:JobState.succeeded) | The job finished successfully. Open its result to see what changed. |
| [Failed](#state:JobState.failed) | The job stopped with an error. Check the last completed step before retrying. |
| [Cancelled](#state:JobState.cancelled) | The job was cancelled. Review what it completed before cancellation. |
| [Lost](#state:JobState.lost) | Orch lost contact with the running job. Check what changed before starting it again. |

### When a database request times out

| Label on screen | What it means |
| --- | --- |
| [Database connection wait expired](#state:DatabaseWaitPhase.acquire) | Orch could not get a database connection in time. Try the request again after a short wait. |
| [Database statement timed out](#state:DatabaseWaitPhase.statement) | The database stopped the operation when it ran out of time. Open Details to see the time limit. |
| [Database transaction expired](#state:DatabaseWaitPhase.idle_transaction) | The database closed an operation that had been waiting too long. Refresh the page to check the current result. |

### Steps in a job

| Label on screen | What it means |
| --- | --- |
| [waiting](#state:PhaseState.pending) | This step has not started yet. Follow the step currently running. |
| [under way](#state:PhaseState.running) | This step is running. Its result appears here when it finishes. |
| [done](#state:PhaseState.done) | This step finished. Read the following steps for the overall outcome. |
| [skipped](#state:PhaseState.skipped) | This step was skipped. Read the job details to see which steps were needed. |

### Search by meaning

| Label on screen | What it means |
| --- | --- |
| [Off](#state:SemanticState.off) | Search by meaning is switched off. You can still search by text. |
| [Not set up](#state:SemanticState.not_named) | Select a provider and model for search by meaning before indexing can begin. |
| [Unavailable](#state:SemanticState.unavailable) | The database cannot currently support search by meaning. Text search remains available. |
| [Ready](#state:SemanticState.idle) | All selected text has been indexed. You can search by meaning now. |
| [Indexing](#state:SemanticState.indexing) | Orch is preparing text for search by meaning. More results become available as work finishes. |
| [Paused](#state:SemanticState.paused) | Indexing is paused. Check the listed reason for storage, provider availability, access or usage limits. |
| [Needs a choice](#state:SemanticState.waiting) | The old and new indexes will not both fit. An administrator must choose how to proceed. |

### Retrying a search update

| Label on screen | What it means |
| --- | --- |
| [Index retry eligible](#state:SemanticRetryNext.immediate) | A smaller request can be tried as soon as capacity allows. You can follow its progress here. |
| [Next indexing pass](#state:SemanticRetryNext.indexer_pass) | Orch will consider the request on the next indexing pass. Leave it queued for that check. |
| [Done](#state:SemanticRetryNext.finished) | The retry finished. Open its result to see whether more work is needed. |

### Exports

| Label on screen | What it means |
| --- | --- |
| [Starting](#state:ExportState.pending) | Your export is being prepared. It will become available to download when ready. |
| [Ready](#state:ExportState.ready) | The export is ready. Download it before it expires. |
| [Failed](#state:ExportState.failed) | The export failed or was cancelled before a file was saved. Check the job before trying again. |
| [Expired](#state:ExportState.expired) | The export file was removed. Create a new export if you need another copy. |

### Imports

| Label on screen | What it means |
| --- | --- |
| [Running](#state:ImportState.pending) | The import is being prepared or run. Open its job to follow progress. |
| [Done](#state:ImportState.done) | The import finished. Read its report for the outcome of each selected item. |
| [Failed](#state:ImportState.failed) | The import stopped with an error. Check its report for items already created. |
| [Cancelled](#state:ImportState.cancelled) | The import was cancelled. Items already created are listed in the report. |
| [Undoing](#state:ImportState.undoing) | Orch is undoing this import. Follow the job until it finishes. |
| [Undone](#state:ImportState.undone) | The items created by this import were taken back. Read the report for details. |

## People and sharing

### Organization setup

| Label on screen | What it means |
| --- | --- |
| [Configured](#state:FirstStepsState.done) | Orch verified this setup step. Continue to the next unfinished step. |
| [Not done yet](#state:FirstStepsState.open) | This setup step still needs attention. Open it to see what to do. |
| [Not known yet](#state:FirstStepsState.unknown) | Orch could not check this step. Check your access and try again. |

### Page access

| Label on screen | What it means |
| --- | --- |
| [Uses its place permissions](#state:PageState.follows) | This page follows its parent's sharing settings. Check the parent to see who can use it. |
| [Custom](#state:PageState.custom) | This page has its own sharing settings. Open sharing on this page to review them. |
| [Private](#state:PageState.restricted) | Access to this page is restricted. Ask its owner if you need access. |

### Who can use a shared resource

| Label on screen | What it means |
| --- | --- |
| [Active for this material and recipient](#state:AgentShareCurrentState.active) | All requested permissions apply to this person here. Open the sharing details to review them. |
| [Reduced by current limits](#state:AgentShareCurrentState.reduced) | Only some requested permissions apply. Check which ones this person can use here. |
| [Inactive now](#state:AgentShareCurrentState.inactive) | This share gives the person no access here. Review other shares if they still have access. |
| [Assessed per material or recipient](#state:AgentShareCurrentState.unassessed) | Access must be checked for the particular person and resource. Select them to see what applies. |

### Files shared with a thread

| Label on screen | What it means |
| --- | --- |
| [waiting to be delivered](#state:AgentShareSnapshotState.pending) | The shared files will be delivered on the thread's next turn. Leave them queued if still needed. |
| [delivered](#state:AgentShareSnapshotState.delivered) | The shared files were written to the thread's working directory. Open the workspace to see them. |
| [taken back](#state:AgentShareSnapshotState.revoked) | The share was withdrawn. Check current access before sharing the files again. |

### Items in the activity history

| Label on screen | What it means |
| --- | --- |
| [Open](#state:AuditObjectState.live) | This item still exists. Open it from the activity history if you have access. |
| [Deleted](#state:AuditObjectState.deleted) | This item was deleted and is being kept for possible recovery. Check its retention details. |
| [Gone on 20 November](#state:AuditObjectState.purged) | Deletion is final. Its activity history still explains what happened. |

## Sign-in

### Account access

| Label on screen | What it means |
| --- | --- |
| [Active](#state:AdminUserStatus.active) | This person can sign in and use the access they have been granted. |
| [Suspended](#state:AdminUserStatus.suspended) | This person's sign-ins and keys have been disabled. Their work stays theirs while access is suspended. |

### Sign-in check

| Label on screen | What it means |
| --- | --- |
| [Pages cannot load Turnstile: the web edge's policy lacks challenges.cloudflare.com. See the operator guide.](#state:HumanCheckEdgeState.blocks) | A web policy is blocking part of the sign-in check. Ask the administrator to allow the hosts named here. |
| [The web edge's policy could not be read from here. See the operator guide.](#state:HumanCheckEdgeState.unknown) | Orch could not read the site's web policy. An administrator can check that the verification widget loads. |
