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

import type { EnvironmentRef, Item, MarkerPayload, UserRef } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { formatBytes, formatNumber } from '@/lib/format';
import { TimeOfDay } from '@/ui/TimeOfDay';
import { Button } from '@/ui/Button';
import { RelativeTime } from '@/ui/RelativeTime';
import { openGiven } from '@/features/inspector/givenFocus';
import { useTimelineContext } from '../context';
import { CallModel } from '@/features/calls/Model';
import { recoveryCause, recoverySentence, RecoveryRuleLink } from '@/features/calls/History';

type MarkerItem = Extract<Item, { kind: 'marker' }>;

const who = (user: UserRef | null, t: TFunction): string => user?.display_name ?? t('timeline.marker.someone');
const environment = (ref: EnvironmentRef | null, t: TFunction): string => ref?.name ?? t('timeline.marker.anEnvironment');

const ICONS: Record<MarkerPayload['marker'], LucideIcon> = {
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

/** The sentence of a line of history. */
export function markerSentence(payload: MarkerPayload, t: TFunction, context: { locale: string; timeZone?: string | undefined }, includeActor = true): string {
  switch (payload.marker) {
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
      return t('timeline.marker.forkedFrom', { by: who(payload.data.by, t), thread: payload.data.thread.name ?? t('timeline.marker.aThread') });
    case 'forked':
      return t('timeline.marker.forked', { by: who(payload.data.by, t), into: payload.data.into.name ?? t('timeline.marker.aThread') });
    case 'environment_recreated':
      return t('timeline.marker.environmentRecreated', { template: payload.data.template.name, reason: t(`timeline.marker.recreated.${payload.data.reason}`) });
    case 'environment_restarted':
      return t('timeline.marker.environmentRestarted',{by:payload.data.by===null?t('environmentRestart.system'):who(payload.data.by,t),name:environment(payload.data.environment,t)});
    case 'environment_moved':
      return t('timeline.marker.environmentMoved', { by: who(payload.data.by, t), from: environment(payload.data.from, t), to: environment(payload.data.to, t) });
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

export function MarkerLine({ item, count = 1 }: { item: MarkerItem; count?: number }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const timeline = useTimelineContext();
  const Icon = ICONS[item.payload.marker];
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
  return (
      <div className="flex items-center justify-center gap-2 py-1 text-center text-sm text-text-subtle">
        <Icon aria-hidden />
        <span className="wrap-anywhere">
          {item.payload.marker === 'call_recovery' && typeof item.payload.data.provider_id === 'string' && typeof item.payload.data.model === 'string' && ['skip', 'return', 'failover'].includes(item.payload.data.action)
            ? <Trans i18nKey={item.payload.data.action === 'skip' ? 'calls.timelineSkippedChip' : item.payload.data.action === 'return' ? 'calls.timelineReturnChip' : 'calls.timelineFailoverChip'}
              values={{ kind: recoveryCause(item.payload.data.kind, t), attempt: formatNumber(item.payload.data.attempt, context.locale), rule: item.payload.data.rule, reason: t(`calls.details.${item.payload.data.detail ?? ''}`, { defaultValue: t('calls.needsAttention', { kind: t('calls.kinds.any') }) }) }}
              components={{ model: <CallModel providerId={item.payload.data.provider_id} model={item.payload.data.model} />, rule: <RecoveryRuleLink rule={item.payload.data.rule} /> }} />
            : markerSentence(item.payload, t, context)}
          {count > 1 && <span className="ml-1">· {t('timeline.marker.more', { count: count - 1 })}</span>}
        </span>
        <TimeOfDay at={item.created_at} className="text-xs" />
      </div>
  );
}
