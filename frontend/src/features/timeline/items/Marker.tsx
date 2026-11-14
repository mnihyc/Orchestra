/** The lines of a thread's history between its items: who created it, changed its model, its page or its owners, switched or forked it, or let a turn past a limit; and segment and lifecycle markers.
 *
 * The lines of a thread's history between its items, each one sentence that names who did what, in
 * the reader's language: "Chen created this thread", "Bo changed the model to gpt-6 at high", "Dana
 * moved Main to host-3". A line is centred with a small icon and has its time under the pointer. A
 * line is one sentence: what it stands for (a plan, a difference, an output) is never in it.
 */
import type { TFunction } from 'i18next';
import {
  Archive,
  ArchiveRestore,
  Box,
  Check,
  CircleDot,
  Cpu,
  Flag,
  GitFork,
  History,
  Info,
  Lock,
  Link2,
  MessageSquareDashed,
  Minimize2,
  OctagonAlert,
  Pause,
  PencilLine,
  Play,
  RefreshCw,
  Settings2,
  ScrollText,
  Share2,
  ShieldCheck,
  Sparkles,
  Square,
  Target,
  UserRound,
  Wrench,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';
import { namedSharedSentence } from '@/features/people/sharedMarker';
import { UserChip } from '@/ui/UserChip';
import { ResourceSentence } from '@/features/resource-facts/ResourceSentence';
import { resourceSubjectOf } from '@/features/resource-facts/facts';

import type { EnvironmentRef, Item, SessionReopenReason, UserRef } from '@/generated/api-types';
import type { MarkerPayload } from '../markerTypes';
import { useFormatContext } from '@/lib/appearance';
import { formatExact, formatSpan } from '@/lib/time';
import { formatBytes, formatNumber } from '@/lib/format';
import { TimeOfDay } from '@/ui/TimeOfDay';
import { Button } from '@/ui/Button';
import { RelativeTime } from '@/ui/RelativeTime';
import { openGiven } from '@/features/inspector/givenFocus';
import { useTimelineContext } from '../context';
import { useThreadPart } from '@/realtime/stores/thread';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import { CallModel } from '@/features/calls/Model';
import { recoveryCause, recoverySentence, RecoveryRuleLink } from '@/features/calls/History';
import { BackgroundWakeMarker } from '@/features/running/BackgroundWakeMarker';
import { environmentLocation } from '@/features/change/jobWords';
import { wakeCauses } from '@/features/running/backgroundWake';

type MarkerItem = Extract<Item, { kind: 'marker' }>;

const who = (user: UserRef | null, t: TFunction): string => user?.display_name ?? t('timeline.marker.someone');
const environment = (ref: EnvironmentRef | null, t: TFunction): string => ref?.name ?? t('timeline.marker.anEnvironment');

/** Why the thread's next turn opened its session again, in words. */
function reopenReason(reason: SessionReopenReason, t: TFunction): string {
  switch (reason) {
    case 'unknown': return t('timeline.marker.sessionReopened.unknown');
    case 'instructions': return t('timeline.marker.sessionReopened.instructions');
    case 'limits': return t('timeline.marker.sessionReopened.limits');
    case 'configuration': return t('timeline.marker.sessionReopened.configuration');
    case 'mcp': return t('timeline.marker.sessionReopened.mcp');
    case 'tools': return t('timeline.marker.sessionReopened.tools');
    case 'skills': return t('timeline.marker.sessionReopened.skills');
    case 'program_reloads': return t('timeline.marker.sessionReopened.programReloads');
    case 'side_history': return t('timeline.marker.sessionReopened.sideHistory');
  }
}

const ICONS: Record<MarkerPayload['marker'], LucideIcon> = {
  harness_changed: Settings2,
  background_wake: Play,
  resource_resumed: Play,
  resource_stopped: Square,
  call_recovery: Play,
  orch_context: ScrollText,
  created: Sparkles,
  renamed: PencilLine,
  described: PencilLine,
  model_changed: Cpu,
  setting_changed: Settings2,
  access_changed: Lock,
  owner_changed: UserRound,
  segment_started: CircleDot,
  agent_history: History,
  switched: GitFork,
  forked_from: GitFork,
  forked: GitFork,
  environment_recreated: Box,
  environment_restarted: Box,
  environment_moved: Box,
  environment_stopped: Square,
  environment_started: Play,
  environment_tools: Wrench,
  capability_granted: ShieldCheck,
  capability_revoked: ShieldCheck,
  compacted: Minimize2,
  instructions_deferred: Info,
  session_reopened: RefreshCw,
  limit_bypassed: Flag,
  archived: Archive,
  unarchived: ArchiveRestore,
  restored: ArchiveRestore,
  shared: Share2,
  unshared: Share2,
  side_started: MessageSquareDashed,
  side_closed: MessageSquareDashed,
  queue_held: Square,
  queue_resumed: Play,
  approval_rules_changed: ShieldCheck,
  thread_moved: Box,
  source_prepared: Box,
  files_restored: ArchiveRestore,
  links_changed: Link2,
  adopted: History,
  goal_set: Target,
  goal_changed: Target,
  goal_paused: Pause,
  goal_resumed: Play,
  goal_met: Check,
  goal_stopped: OctagonAlert,
  goal_cleared: Target,
};

const nativeRecord=(value:unknown):value is Record<string,unknown>=>typeof value==='object' && value!==null && !Array.isArray(value);

/** Native enum words are interpreted here; instruction text never reaches this projection. */
export function nativeChangeValue(change:string,value:unknown,t:TFunction):string {
  const named=(name:string)=>t(`harnessChanges.values.${name}` as 'harnessChanges.values.default');
  if(change==='environment_connection' && (value==='connected' || value==='disconnected'))return t(value==='connected'?'harnessChanges.environmentConnected':'harnessChanges.environmentDisconnected');
  if(change==='reply_buffering' && nativeRecord(value) && typeof value.show_ui==='boolean')return t(value.show_ui?'harnessChanges.bufferingVisible':'harnessChanges.bufferingHidden');
  if(change==='system_event' && nativeRecord(value)) {
    const kind=typeof value.kind==='string' ? value.kind : '';
    const label=t(`harnessChanges.events.${kind}` as 'harnessChanges.events.informational',{defaultValue:t('harnessChanges.programEvent')});
    const words=[value.title,value.message,value.reason,value.hook_name].filter((part):part is string=>typeof part==='string' && part.length>0);
    return [label,...new Set(words)].join(t('harnessChanges.separator'));
  }
  if(change==='mcp_startup' && nativeRecord(value)) {
    const status=typeof value.status==='string' ? value.status : '';
    const words=t(`harnessChanges.mcpStates.${status}` as 'harnessChanges.mcpStates.failed',{defaultValue:t('harnessChanges.values.custom')});
    const reason=value.failure_reason==='reauthenticationRequired' ? t('harnessChanges.mcpSignIn') : null;
    return [words,reason,typeof value.message==='string'?value.message:null].filter((part):part is string=>part!==null).join(t('harnessChanges.separator'));
  }
  if(change==='session_status' && nativeRecord(value)) {
    const state=typeof value.type==='string' ? value.type : '';
    const parts=[t(`harnessChanges.sessionStates.${state}` as 'harnessChanges.sessionStates.idle',{defaultValue:t('harnessChanges.values.custom')})];
    if(Array.isArray(value.active_flags))for(const flag of value.active_flags) {
      if(flag==='waitingOnApproval')parts.push(t('harnessChanges.waitingApproval'));
      if(flag==='waitingOnUserInput')parts.push(t('harnessChanges.waitingInput'));
    }
    return parts.join(t('harnessChanges.separator'));
  }
  if(change==='managed_setting') {
    if(typeof value==='boolean')return named(value?'on':'off');
    if(typeof value==='string' || typeof value==='number')return String(value);
    return t('harnessChanges.notReported');
  }
  if(change==='managed_settings_readback')return t('harnessChanges.notReported');
  if(change==='collaboration_instructions')return named(value===null?'noAddedInstructions':'privateInstructions');
  if(value===null)return named('default');
  if(change==='model' && typeof value==='string')return modelDisplayName(value,t);
  if(change==='effort' && typeof value==='string')return effortDisplayName(value,t);
  if(change==='approval_mode' && nativeRecord(value)) {
    if(typeof value.native_mode==='string') {
      const modes:Record<string,string>={default:'ask',acceptEdits:'onRequest',auto:'automatic',bypassPermissions:'bypass',plan:'plan'};
      const mode=modes[value.native_mode];
      return mode===undefined ? value.native_mode : named(mode);
    }
    const policy=value.policy;
    if(policy==='never')return named('bypass');
    if(value.reviewer==='auto_review')return named('automatic');
    if(policy==='untrusted')return named('ask');
    if(policy==='on-request')return named('onRequest');
    if(policy==='on-failure')return named('onFailure');
    return named('custom');
  }
  if(change==='sandbox_mode' && nativeRecord(value)) {
    const modes:Record<string,string>={readOnly:'readOnly',workspaceWrite:'workspace',dangerFullAccess:'open',externalSandbox:'external'};
    const mode=typeof value.type==='string' ? modes[value.type] : undefined;
    const parts=[mode===undefined ? named('custom') : named(mode)];
    if(typeof value.networkAccess==='boolean')parts.push(named(value.networkAccess?'networkAllowed':'networkBlocked'));
    if(Array.isArray(value.writableRoots)) {
      const roots=value.writableRoots.filter((root):root is string=>typeof root==='string');
      parts.push(t('harnessChanges.roots',{roots:roots.length===0?named('noFoldersListed'):roots.join(', ')}));
    }
    if(typeof value.excludeTmpdirEnvVar==='boolean')parts.push(named(value.excludeTmpdirEnvVar?'systemTmpExcluded':'systemTmpIncluded'));
    if(typeof value.excludeSlashTmp==='boolean')parts.push(named(value.excludeSlashTmp?'tmpExcluded':'tmpIncluded'));
    return parts.join(t('harnessChanges.separator'));
  }
  if(change==='collaboration_mode' && nativeRecord(value)) {
    const parts=[named(value.mode==='plan'?'plan':value.mode==='default'?'work':'custom')];
    if(nativeRecord(value.settings)) {
      if(typeof value.settings.model==='string')parts.push(modelDisplayName(value.settings.model,t));
      if(typeof value.settings.reasoning_effort==='string')parts.push(effortDisplayName(value.settings.reasoning_effort,t));
    }
    return parts.join(t('harnessChanges.separator'));
  }
  if(typeof value==='boolean')return named(value?'on':'off');
  if(typeof value==='string' || typeof value==='number')return String(value);
  return named('custom');
}

/** The sentence of a line of history. */
export function markerSentence(payload: MarkerPayload, t: TFunction, context: { locale: string; timeZone?: string | undefined; providerName?: string | undefined }, includeActor = true): string {
  switch (payload.marker) {
    case 'harness_changed': {
      const data=payload.data;
      if(data.change==='rate_limit') {
        const value=nativeRecord(data.value)?data.value:{};
        const words=[value.message,value.reason].filter((part):part is string=>typeof part==='string' && part.length>0);
        const status=value.status;
        const fallback=status==='allowed'?t('harnessChanges.rateLimitStates.allowed'):status==='allowed_warning'?t('harnessChanges.rateLimitStates.warning'):status==='rejected'?t('harnessChanges.rateLimitStates.rejected'):typeof status==='string'?status:t('harnessChanges.rateLimitStates.updated');
        return t('harnessChanges.rateLimit',{provider:context.providerName === undefined || context.providerName === '' ? t('harnessChanges.provider') : context.providerName,message:words.length>0?[...new Set(words)].join(t('harnessChanges.separator')):fallback});
      }
      if(data.change==='unrecognized_event' && typeof data.subject==='string') {
        const count=nativeRecord(data.value)?data.value.count:undefined;
        return typeof count==='number' && Number.isFinite(count) && count>1
          ? t('harnessChanges.unrecognizedCount',{program:data.program,name:data.subject,number:formatNumber(count,context.locale)})
          : t('harnessChanges.unrecognized',{program:data.program,name:data.subject});
      }
      if(data.change==='context_compaction')return t('timeline.marker.compacted');
      if(data.change==='managed_settings_readback')return t(nativeRecord(data.value) && data.value.status==='mismatch' ? 'harnessChanges.pinsDiffer' : nativeRecord(data.value) && data.value.status==='reported' ? 'harnessChanges.pinsReported' : 'harnessChanges.pinsIncomplete',{program:data.program});
      if(data.change==='system_event')return t('harnessChanges.event',{program:data.program,value:nativeChangeValue(data.change,data.value,t)});
      if(data.change==='hook' && nativeRecord(data.value)) {
        const status=data.value.status;
        const name=data.subject===null || data.subject===undefined ? null : t(`harnessChanges.hookEvents.${data.subject}` as 'harnessChanges.hookEvents.preToolUse',{defaultValue:data.subject});
        const key=status==='running'?'hookRunning':status==='failed'?'hookFailed':status==='blocked'?'hookBlocked':status==='stopped'?'hookStopped':status==='completed'?'hookRan':'hookReported';
        return name===null ? t(`harnessChanges.${key}Unnamed`) : t(`harnessChanges.${key}`,{name});
      }
      if(data.change==='session_lifecycle' && data.value==='closed')return t('harnessChanges.closed',{program:data.program});
      if(data.change==='session_lifecycle' && data.value==='resumed')return t('harnessChanges.resumed',{program:data.program});
      const program=data.subject===undefined || data.subject===null ? data.program : t('harnessChanges.subject',{program:data.program,subject:data.subject});
      const changed=data.changed;
      const line=t(changed?'harnessChanges.changed':'harnessChanges.reported',{program,setting:t(`harnessChanges.settings.${data.change}` as 'harnessChanges.settings.model',{defaultValue:t('harnessChanges.setting')}),value:nativeChangeValue(data.change,data.value,t)});
      return data.next_turn ? `${line} ${t('harnessChanges.future')}` : line;
    }
    case 'background_wake':
      return t(payload.data.admitted ? 'backgroundWake.wokenBy' : 'backgroundWake.waitingBy', { causes: wakeCauses(payload.data.causes, t) || t('backgroundWake.work') });
    case 'resource_resumed':
      return t(payload.data.input_id === null ? 'storage.resumed.refused' : 'storage.resumed.continued', { name: payload.data.name, by: who(payload.data.by, t) });
    case 'resource_stopped':
      return t(payload.data.deleted ? 'storage.stopped.deleted' : 'storage.stopped.disabled', { name: payload.data.name, by: who(payload.data.by, t) });
    case 'call_recovery':
      if (payload.data.action === 'continue' || payload.data.action === 'retry') return recoverySentence(payload.data, t, context.locale);
      if (payload.data.action === 'skip') return t('calls.timelineSkipped', { model: payload.data.model ?? '', rule: payload.data.rule, reason: t(`calls.details.${payload.data.detail ?? ''}`, { defaultValue: t('calls.needsAttention', { kind: t('calls.kinds.any') }) }) });
      if (payload.data.action === 'return') return t('calls.timelineReturn', { model: payload.data.model ?? '', rule: payload.data.rule });
      if (payload.data.action === 'failover' && typeof payload.data.model === 'string') return t('calls.timelineFailover', { model: payload.data.model, kind: recoveryCause(payload.data.kind, t), attempt: formatNumber(payload.data.attempt, context.locale), rule: payload.data.rule });
      return t('calls.timeline', { action: t(`calls.actions.${payload.data.action}`, { defaultValue: t('calls.status') }), kind: recoveryCause(payload.data.kind, t), attempt: formatNumber(payload.data.attempt, context.locale), rule: payload.data.rule });
    case 'agent_history': {
      const data = payload.data;
      const text = data.recovered === null
        ? t(data.by === null ? 'agentHistory.removed' : 'agentHistory.removedBy', { program: data.program })
        : t('agentHistory.recovered', { program: data.program, history: t(`agentHistory.history.${data.recovered}`) });
      const actor = includeActor && data.recovered === null ? `${data.by?.display_name ?? t('agentHistory.automatic')} · ` : '';
      return `${actor}${text} · ${data.bytes === null ? '–' : formatBytes(data.bytes, context.locale)}`;
    }
    case 'orch_context':
      return t('threadViews.given.context');
    case 'created':
      return t('timeline.marker.created', { by: who(payload.data.by, t) });
    case 'renamed':
      return payload.data.by === null
        ? t('timeline.marker.renamedAuto', { to: payload.data.to })
        : t('timeline.marker.renamed', { by: who(payload.data.by, t), to: payload.data.to });
    case 'described':
      return t('timeline.marker.described', { by: who(payload.data.by, t) });
    case 'model_changed': {
      const { by, to, effort } = payload.data;
      const model = to;
      if (effort.to !== null && effort.to !== effort.from) return t('timeline.marker.modelChangedEffort', { by: who(by, t), model, effort: effort.to });
      return t('timeline.marker.modelChanged', { by: who(by, t), model });
    }
    case 'setting_changed':
      return t('timeline.marker.settingChanged', { by: who(payload.data.by, t), label: payload.data.label });
    case 'access_changed':
      return t('access.history.words.access', { actor: who(payload.data.by, t), name: t('access.history.thisThread') });
    case 'owner_changed':
      return t('timeline.marker.ownerChanged', { by: who(payload.data.by, t), to: payload.data.to.display_name, from: payload.data.from.display_name });
    case 'segment_started':
      return payload.data.resumed
        ? t('timeline.marker.segmentResumed', { agent: payload.data.agent.name, provider: payload.data.provider.name })
        : t('timeline.marker.segmentStarted', { agent: payload.data.agent.name, provider: payload.data.provider.name });
    case 'switched': {
      // The sentence says what the new session was given, so a reader sees why it may not know the earlier turns.
      const words = { by: who(payload.data.by, t), provider: payload.data.to.provider.name, agent: payload.data.to.agent.name };
      const report = payload.data.report;
      const carried = Object.values(report.by_kind).reduce((total, kind) => total + kind.kept + kind.converted, 0);
      if (carried === 0) return t('timeline.marker.switchedFresh', words);
      if (report.fidelity === 'summary') return t('timeline.marker.switchedSummary', words);
      if (report.fidelity === 'lossless') return t('timeline.marker.switched', words);
      return t('timeline.marker.switchedCarried', { ...words, count: carried });
    }
    case 'forked_from':
      return t(payload.data.by_agent === undefined || payload.data.by_agent === null ? 'timeline.marker.forkedFrom' : 'timeline.marker.agentForkedFrom', { by: who(payload.data.by, t), thread: payload.data.thread.name ?? t('timeline.marker.aThread') });
    case 'forked':
      return t(payload.data.by_agent === undefined || payload.data.by_agent === null ? 'timeline.marker.forked' : 'timeline.marker.agentForked', { by: who(payload.data.by, t), into: payload.data.into.name ?? t('timeline.marker.aThread') });
    case 'environment_recreated':
      return t('timeline.marker.environmentRecreated', { template: payload.data.template.name, reason: t(`timeline.marker.recreated.${payload.data.reason}`) });
    case 'environment_restarted':
      return t('timeline.marker.environmentRestarted',{by:payload.data.by===null?t('environmentRestart.system'):who(payload.data.by,t),name:environment(payload.data.environment,t)});
    case 'environment_moved': {
      const data = payload.data;
      const located = (ref: EnvironmentRef, host: typeof data.from_host, machine: typeof data.from_machine): string => {
        const location = environmentLocation(host, machine);
        return location === null ? t('timeline.marker.environmentLocationUnrecorded', { environment: environment(ref, t) }) : t('timeline.marker.environmentOn', { environment: environment(ref, t), location });
      };
      const moved = t('timeline.marker.environmentMoved', { by: who(data.by, t), from: located(data.from, data.from_host, data.from_machine), to: located(data.to, data.to_host, data.to_machine) });
      return data.mounts_detached.length === 0 ? moved : `${moved} · ${t('timeline.marker.mountsDetached', { mounts: data.mounts_detached.join(t('startFlow.separator')) })}`;
    }
    case 'environment_stopped':
      return payload.data.by === null
        ? t('timeline.marker.environmentStoppedBecause', { name: environment(payload.data.environment, t), reason: t(`timeline.marker.stopped.${payload.data.reason}`) })
        : t('timeline.marker.environmentStopped', { by: who(payload.data.by, t), name: environment(payload.data.environment, t) });
    case 'environment_started':
      return payload.data.by === null
        ? t('timeline.marker.environmentStartedAuto', { name: environment(payload.data.environment, t) })
        : t('timeline.marker.environmentStarted', { by: who(payload.data.by, t), name: environment(payload.data.environment, t) });
    case 'environment_tools':
      return t('access.history.words.tools', { actor: who(payload.data.by, t), name: t('access.history.thisThread') });
    case 'capability_granted':
      return payload.data.by === null
        ? t('timeline.marker.capabilityAllowed', { capability: payload.data.capability })
        : t('timeline.marker.capabilityGranted', { by: who(payload.data.by, t), capability: payload.data.capability });
    case 'capability_revoked':
      return t('timeline.marker.capabilityRevoked', { by: who(payload.data.by, t), capability: payload.data.capability });
    case 'compacted':
      return payload.data.duration_ms === null
        ? t('timeline.marker.compacted')
        : t('timeline.marker.compactedIn', { duration: formatSpan(payload.data.duration_ms, context) });
    case 'instructions_deferred':
      return t('timeline.marker.instructionsDeferred', { program: payload.data.program });
    case 'session_reopened':
      return reopenReason(payload.data.reason, t);
    case 'limit_bypassed':
      return t('timeline.marker.limitBypassed', { by: who(payload.data.by, t), limits: payload.data.limits.map((limit) => limit.label).join(', ') });
    case 'archived':
      return t('timeline.marker.archived', { by: who(payload.data.by, t), node: payload.data.node.name ?? t('timeline.marker.aPlace') });
    case 'unarchived':
      return t('timeline.marker.unarchived', { by: who(payload.data.by, t), node: payload.data.node.name ?? t('timeline.marker.aPlace') });
    case 'restored':
      return t('timeline.marker.restored', { by: who(payload.data.by, t), node: payload.data.node.name ?? t('timeline.marker.aPlace') });
    case 'shared':
      if (payload.data.parts !== null && payload.data.parts !== undefined) return namedSharedSentence(payload.data, t) ?? t('timeline.marker.shared', { by: who(payload.data.by, t) });
      if ('document' in payload.data.what) return t('timeline.marker.sharedDocument', { by: who(payload.data.by, t), title: payload.data.what.document.title });
      return t('timeline.marker.shared', { by: who(payload.data.by, t) });
    case 'unshared':
      if ('document' in payload.data.what) return t('timeline.marker.unsharedDocument', { by: who(payload.data.by, t), title: payload.data.what.document.title });
      return t('timeline.marker.unshared', { by: who(payload.data.by, t) });
    case 'side_started':
      return t('timeline.marker.sideStarted', { by: who(payload.data.by, t) });
    case 'side_closed':
      return t('timeline.marker.sideClosed', { by: who(payload.data.by, t), count: payload.data.turns });
    case 'queue_held':
      return t('timeline.marker.queueHeld', { by: who(payload.data.by, t), count: payload.data.inputs });
    case 'queue_resumed':
      return payload.data.inputs === 0
        ? t('timeline.marker.queueReleased')
        : t('timeline.marker.queueResumed', { by: who(payload.data.by, t), count: payload.data.inputs });
    case 'approval_rules_changed':
      if (payload.data.action === 'allowed') return t('timeline.marker.ruleAllowed', { by: who(payload.data.by, t) });
      return payload.data.by === null ? t('timeline.marker.ruleEnded') : t('timeline.marker.ruleRevoked', { by: who(payload.data.by, t) });
    case 'thread_moved':
      return t('threadMove.history', { by: who(payload.data.by, t), from: payload.data.from.name ?? t('timeline.marker.aPlace'), to: payload.data.to.name ?? t('timeline.marker.aPlace') });
    case 'source_prepared':
      return t('timeline.marker.sourcePrepared', { by: who(payload.data.by, t), directory: payload.data.directory });
    case 'files_restored':
      return t('timeline.marker.filesRestored', { by: who(payload.data.by, t), count: payload.data.files });
    case 'links_changed':
      return t('timeline.marker.linksChanged', { by: who(payload.data.by, t), count: payload.data.count });
    case 'goal_set':
      return t('goals.marker.set', { by: includeActor ? who(payload.data.by, t) : '', condition: payload.data.condition });
    case 'goal_changed':
      return t('goals.marker.changed', { by: includeActor ? who(payload.data.by, t) : '', condition: payload.data.condition });
    case 'goal_paused':
      switch (payload.data.reason) {
        case 'interrupted': return t('goals.marker.pausedInterrupted', { by: includeActor ? who(payload.data.by, t) : '' });
        case 'blocked': return t('goals.marker.pausedBlocked');
        case 'failed': return t('goals.marker.pausedFailed');
        default: return t('goals.marker.pausedBy', { by: includeActor ? who(payload.data.by, t) : '' });
      }
    case 'goal_resumed':
      return t('goals.marker.resumed', { by: includeActor ? who(payload.data.by, t) : '' });
    case 'goal_met':
      return t(payload.data.checked_by === 'agent' ? 'goals.marker.metAgent' : 'goals.marker.met', { count: payload.data.turns });
    case 'goal_stopped':
      return t('goals.marker.stopped', { reason: t(`goals.reason.${payload.data.reason}`, { limit: payload.data.limit ?? '' }) });
    case 'goal_cleared':
      return t('goals.marker.cleared', { by: includeActor ? who(payload.data.by, t) : '' });
    case 'adopted': {
      // Who took the session in and how much of what it held before came, then the model where a person chooses one.
      const taken =
        payload.data.history_missing === null
          ? t('timeline.marker.adopted', { by: who(payload.data.by, t), count: payload.data.turns })
          : t(`timeline.marker.adoptedWithout.${payload.data.history_missing}`, { by: who(payload.data.by, t) });
      return payload.data.model_reason === null
        ? taken
        : `${taken} · ${t(`timeline.marker.adoptedModel.${payload.data.model_reason}`, { model: payload.data.session_model ?? '' })}`;
    }
  }
}

/** A goal that was met says so in one sentence and keeps the evidence behind a button, as the context line keeps its record. */
function GoalMetLine({ item, payload, Icon }: { item: MarkerItem; payload: Extract<MarkerPayload, { marker: 'goal_met' }>; Icon: LucideIcon }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const [shown, setShown] = useState(false);
  return (
    <div className="flex flex-col items-center gap-1 py-1 text-center text-sm text-text-subtle" data-goal-met>
      <div className="flex flex-wrap items-center justify-center gap-2">
        <Icon aria-hidden />
        <span className="wrap-anywhere">{markerSentence(item.payload, t, context)}</span>
        <Button size="sm" variant="ghost" aria-expanded={shown} onClick={() => setShown((held) => !held)}>{t(shown ? 'goals.marker.evidenceLess' : 'goals.marker.evidenceMore')}</Button>
        <TimeOfDay at={item.created_at} className="text-xs" />
      </div>
      {shown && <p className="max-w-full rounded-md bg-bg-muted px-3 py-2 text-start text-sm whitespace-pre-wrap text-text-muted wrap-anywhere" data-goal-evidence>{payload.data.evidence}</p>}
    </div>
  );
}

function RateLimitMarker({item,threadId}:{item:MarkerItem;threadId:string}) {
  const {t}=useTranslation();
  const context=useFormatContext();
  // The historical segment's already bounded provider name; a later switch cannot rename this notice.
  const providerName=useThreadPart(threadId,view=>view.segments.find(segment=>segment.id===item.segment_id)?.provider?.name);
  return <div title={formatExact(item.created_at,context)} className="flex items-center justify-center gap-2 py-1 text-center text-sm text-text-subtle">
    <Settings2 aria-hidden/><span className="wrap-anywhere">{markerSentence(item.payload,t,{...context,providerName})}</span>
  </div>;
}

export function MarkerLine({ item, count = 1 }: { item: MarkerItem; count?: number }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const timeline = useTimelineContext();
  if(item.payload.marker==='harness_changed' && item.payload.data.change==='rate_limit')return <RateLimitMarker item={item} threadId={timeline.threadId}/>;
  const Icon = ICONS[item.payload.marker];
  if (item.payload.marker === 'background_wake') return <BackgroundWakeMarker threadId={timeline.threadId} data={item.payload.data} at={item.created_at} />;
  if (item.payload.marker === 'resource_resumed') {
    const data=item.payload.data;
    const reason=data.reason==='original_author_unavailable'||data.reason==='original_input_unavailable'?t(`storage.resumed.reasons.${data.reason}`):null;
    const resource=data.model === null ? resourceSubjectOf(data.object,data.name) : {kind:'model' as const,providerId:data.object.id,model:{model_name:data.model,display_name:data.name,parameters:{}}};
    return <div className="flex flex-wrap items-center justify-center gap-1 text-sm text-text-muted"><Icon aria-hidden />
      {resource === null ? <span>{markerSentence(item.payload,t,context)}</span> : <ResourceSentence text={t('storage.resumed.resource',{name:data.name})} resources={[{label:data.name,subject:resource}]} />}
      <UserChip user={data.by} size={24} /><span>· {t(data.input_id === null ? 'storage.resumed.stoppedTask' : 'storage.resumed.continuedTask')}</span>
      {reason !== null && <span className="text-warning-text">· {reason}</span>}<TimeOfDay at={item.created_at} />
    </div>;
  }
  if (item.payload.marker === 'resource_stopped') {
    const data=item.payload.data;
    const resource=data.model === null ? resourceSubjectOf(data.object,data.name) : {kind:'model' as const,providerId:data.object.id,model:{model_name:data.model,display_name:data.name,parameters:{}}};
    return <div className="flex flex-wrap items-center justify-center gap-1 text-sm text-text-muted"><Icon aria-hidden />
      {resource === null ? <span>{markerSentence(item.payload,t,context)}</span> : <ResourceSentence text={t(data.deleted ? 'storage.stopped.deletedResource' : 'storage.stopped.disabledResource',{name:data.name})} resources={[{label:data.name,subject:resource}]} />}
      <UserChip user={data.by} size={24} />{data.note !== null && <span className="min-w-0 truncate" title={data.note}>· {data.note}</span>}<TimeOfDay at={item.created_at} />
    </div>;
  }
  if (item.payload.marker === 'call_recovery' && ['continue', 'retry'].includes(item.payload.data.action)) {
    return <div className="min-w-0 wrap-anywhere py-1 text-center text-sm text-text-subtle" data-recovery-event>
      <Icon className="mr-1 inline-block size-4 align-text-bottom" aria-hidden />
      <span>{recoverySentence(item.payload.data, t, context.locale, false)} · <RecoveryRuleLink rule={item.payload.data.rule} /></span>
      <span className="whitespace-nowrap"> · <TimeOfDay at={item.created_at} className="text-xs" /></span>
    </div>;
  }
  if (item.payload.marker === 'agent_history') {
    const data = item.payload.data;
    return <div data-agent-history={item.id} className="flex flex-wrap items-center justify-center gap-2 py-1 text-center text-sm text-text-muted">
      <Icon aria-hidden />
      {data.recovered === null && (data.by === null ? <span>{t('agentHistory.automatic')}</span> : <UserChip user={data.by} />)}
      <span className="wrap-anywhere">{markerSentence(item.payload, t, context, false)}</span>
      {data.recovered !== null && data.removed_at !== null && <span>{t('agentHistory.removedAt')} <RelativeTime at={data.removed_at} /></span>}
      <RelativeTime at={item.created_at} />
    </div>;
  }
  // A session context can precede the first turn. Keep that orphan link in one compact line;
  // turn-bound contexts are grouped in their footer by rows.ts.
  if (item.payload.marker === 'orch_context') {
    const record = item.payload.data.model_input_id;
    return <div data-context-marker className="flex items-center gap-2 py-1 text-xs text-text-muted"><Icon aria-hidden />
      <Button size="sm" variant="ghost" title={t('threadViews.given.received')} onClick={() => { openGiven(timeline.threadId, { record }); }}>{t('threadViews.given.context')}</Button>
      <TimeOfDay at={item.created_at} />
    </div>;
  }
  if (item.payload.marker === 'goal_met') return <GoalMetLine item={item} payload={item.payload} Icon={Icon} />;
  if (item.payload.marker === 'goal_set' || item.payload.marker === 'goal_changed' || item.payload.marker === 'goal_paused'
    || item.payload.marker === 'goal_resumed' || item.payload.marker === 'goal_cleared') {
    const by = item.payload.data.by;
    return <div className="flex flex-wrap items-center justify-center gap-2 py-1 text-center text-sm text-text-muted" data-goal-marker={item.payload.marker}>
      <Icon aria-hidden />
      {by !== null && <UserChip user={by} />}
      <span className="wrap-anywhere">{markerSentence(item.payload, t, context, by === null).trim()}</span>
      <TimeOfDay at={item.created_at} className="text-xs" />
    </div>;
  }
  // A line of history has no time of its own beside it: the moment is under the pointer.
  return (
      <div title={formatExact(item.created_at, context)} className="flex items-center justify-center gap-2 py-1 text-center text-sm text-text-subtle">
        <Icon aria-hidden />
        <span className="wrap-anywhere">
          {item.payload.marker === 'call_recovery' && typeof item.payload.data.provider_id === 'string' && typeof item.payload.data.model === 'string' && ['skip', 'return', 'failover'].includes(item.payload.data.action)
            ? <Trans i18nKey={item.payload.data.action === 'skip' ? 'calls.timelineSkippedChip' : item.payload.data.action === 'return' ? 'calls.timelineReturnChip' : 'calls.timelineFailoverChip'}
              values={{ kind: recoveryCause(item.payload.data.kind, t), attempt: formatNumber(item.payload.data.attempt, context.locale), rule: item.payload.data.rule, reason: t(`calls.details.${item.payload.data.detail ?? ''}`, { defaultValue: t('calls.needsAttention', { kind: t('calls.kinds.any') }) }) }}
              components={{ model: <CallModel providerId={item.payload.data.provider_id} model={item.payload.data.model} />, rule: <RecoveryRuleLink rule={item.payload.data.rule} /> }} />
            : markerSentence(item.payload, t, context)}
          {count > 1 && <span className="ml-1">· {t('timeline.marker.more', { count: count - 1 })}</span>}
        </span>
      </div>
  );
}
