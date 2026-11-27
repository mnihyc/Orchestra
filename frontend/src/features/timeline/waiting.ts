/** What a thread is waiting for, in words: the step of its start, the reason it is blocked, the job that holds it.
 *
 * The same words stand under the input that waits, in the state chip of the header and in the status
 * line that ends the conversation, so they are made once, here.
 */
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';

import type { Thread, Turn } from '@/generated/api-types';
import { stoppingSince } from '@/lib/threadStopping';
import { formatBytes, formatNumber } from '@/lib/format';
import { i18n } from '@/i18n';
import { sandboxWords } from '@/features/resources/SandboxReadiness';
import { preparationFailure, sourceFailure } from '@/features/source/words';

const APPROVAL_KEYS=['threadConfinement.approvalNotAllowed','threadConfinement.approvalUnsupported'] as const;
const NAMED_APPROVAL_KEYS=['threadConfinement.approvalNotAllowedMode','threadConfinement.approvalUnsupportedMode'] as const;
const AUTO_APPROVAL_KEYS=['threadConfinement.autoNotProven','composer.approvalUnavailable.autoNeedsHost','threadConfinement.autoCheckFailed','threadConfinement.autoCheckUnavailable'] as const;
const CONFINEMENT_REFUSAL_KEYS=[...APPROVAL_KEYS,...AUTO_APPROVAL_KEYS,'threadConfinement.sandboxUnsupported'] as const;
const CONFINEMENT_KEYS=[...CONFINEMENT_REFUSAL_KEYS,'agentHistory.removingDetail','agentHistory.sideNeedsImport'] as const;
const MACHINE_PROGRAM_VERSION = /^Codex on (.+) is ([^;\r\n]+); this agent runs ([^\r\n]+)\. An environment manager can allow it on the environment's page\.$/u;
export const NATIVE_END_UNCONFIRMED = "native_turn_still_running: the native turn's end is not known; fresh work waits for its actual completion";
/** The engine's whole sentence when a removal of the environment takes the thread out of it. */
const ENVIRONMENT_REMOVED = 'Its environment was removed.';

/** A native deleted-provider outcome is distinct from a temporary refusal or a disabled provider. */
export function isProviderRemoved(thread: Thread): boolean {
  return thread.state === 'blocked' && thread.blocked?.reason === 'provider_unavailable' && thread.blocked.detail === 'deleted';
}

/** Only this complete engine sentence names the environment's current machine-program version refusal. */
export function isMachineProgramVersionMismatch(raw: string): boolean {
  return MACHINE_PROGRAM_VERSION.test(raw);
}
const APPROVAL_MODES=[['Ask','ask'],['On request','on_request'],['Auto','auto'],['Bypass','bypass']] as const;

/** Match only complete current product explanations; a vendor detail is never rewritten. */
function confinementDetail(raw: string, t: TFunction): string | null {
  const english = i18n.getFixedT('en');
  if (raw.startsWith('Approval mode “')) for (const [name, mode] of APPROVAL_MODES) {
    const key = NAMED_APPROVAL_KEYS.find((key) => raw === english(key, { mode: name }));
    if (key !== undefined) return t(key, { mode: t(`composer.mode.labels.${mode}`) });
  }
  const key = CONFINEMENT_REFUSAL_KEYS.find((key) => english(key) === raw);
  return key === undefined ? null : t(key);
}

/** This automatic native-cache wait has no human recovery action and is not an error. */
export function isAgentHistoryRemoving(thread: Thread): boolean {
  return thread.state === 'blocked' && thread.blocked?.detail === i18n.getFixedT('en')('agentHistory.removingDetail');
}

/** The source owns its state and reason; prose is never a protocol for a failed preparation. */
function sourceFailedWords(source: Thread['source'], t: TFunction): string | null {
  return source?.state === 'failed' ? preparationFailure(t, source) : null;
}

/** Only known engine explanations are translated; unknown/native error details remain verbatim. */
/** Why an environment failed, from the server's sentence for a thread it blocks ("the environment failed: <step>: <why>"),
 * without the step the job stopped in (its progress shows the steps) and without a closing stop; null when none is given. */
export function failedEnvironmentWhy(raw: string): string | null {
  const why = /^the environment failed: (?:(?:schedule|image|volumes|tools|create|enroll|attach|files|after_up|verify|commit|start|start_enroll|start_attach|start_verify|start_commit): )?(.+)$/su.exec(raw)?.[1]?.trim().replace(/[.。]+$/u, '');
  return why === undefined || why === '' ? null : why;
}

export function blockedDetail(raw:string,t:TFunction,source: Thread['source'] = null):string {
  const sourceFailed = sourceFailedWords(source, t);
  if (sourceFailed !== null) return sourceFailed;
  const programVersion = MACHINE_PROGRAM_VERSION.exec(raw);
  if (programVersion?.[1] !== undefined && programVersion[2] !== undefined && programVersion[3] !== undefined) {
    return t('environmentTools.versionMismatch', { machine: programVersion[1],
      programVersion: programVersion[2] === 'unknown' ? t('environmentTools.unknownVersion') : programVersion[2],
      agentVersion: programVersion[3] === 'unknown' ? t('environmentTools.unknownVersion') : programVersion[3] });
  }
  // Retained blocked rows from the previous engine used these three exact internal sentences.
  if (['ReadOnly','Workspace','Open'].some((mode)=>raw===`no sandbox mode at or below ${mode} can be enforced here: change the setting, the environment's limit or the environment`)) {
    return t('threadConfinement.sandboxUnsupported');
  }
  // Retained rows can predate the descriptor-aware approval default.
  if (/^the approval mode (Ask|OnRequest|Auto|Bypass) is not among those allowed here$/u.test(raw)) {
    return t('threadConfinement.approvalNotAllowed');
  }
  if (/^the agent does not have the approval mode (Ask|OnRequest|Auto|Bypass)$/u.test(raw)) {
    return t('threadConfinement.approvalUnsupported');
  }
  const refused = /^(?:harness_unavailable|harness_error): (.+)$/su.exec(raw);
  if (refused?.[1] !== undefined) return t('threadConfinement.agentRefused', { detail: refused[1] });
  const english=i18n.getFixedT('en');
  const confinement = confinementDetail(raw, t);
  if (confinement !== null) return confinement;
  const key=CONFINEMENT_KEYS.find((key)=>english(key)===raw);
  return key===undefined?raw:t(key);
}

/** What a job that holds the thread is, in words. */
function lockWords(kind: string, t: TFunction): string {
  switch (kind) {
    case 'thread_fork':
      return t('thread.lock.kind.thread_fork');
    case 'environment_move':
      return t('thread.lock.kind.environment_move');
    case 'environment_update':
      return t('thread.lock.kind.environment_update');
    case 'environment_recreate':
      return t('thread.lock.kind.environment_recreate');
    default:
      return t('thread.lock.kind.other');
  }
}

/** What a kind of command that a start waits for is doing, in words; null for a kind that has none. */
function startWork(command: string, t: TFunction): string | null {
  switch (command) {
    case 'fs.stat': return t('thread.starting.work.fsStat');
    case 'fs.mkdir': return t('thread.starting.work.fsMkdir');
    case 'git.prepare': return t('thread.starting.work.gitPrepare');
    case 'git.worktree': return t('thread.starting.work.gitWorktree');
    case 'fs.record': return t('thread.starting.work.fsRecord');
    case 'fs.materialize': return t('thread.starting.work.fsMaterialize');
    case 'fs.materialize_if_absent': return t('thread.starting.work.fsNotesCopy');
    case 'session.open': return t('thread.starting.work.sessionOpen');
    default: return null;
  }
}

/** The other work that holds a start up, as a noun, so that it reads after "waiting behind". */
function otherWork(command: string, t: TFunction): string {
  switch (command) {
    case 'fs.record': return t('thread.starting.other.fsRecord');
    case 'git.prepare': return t('thread.starting.other.gitPrepare');
    case 'git.worktree': return t('thread.starting.other.gitWorktree');
    case 'fs.materialize': return t('thread.starting.other.fsMaterialize');
    case 'fs.materialize_if_absent': return t('thread.starting.other.fsNotesCopy');
    case 'fs.snapshot': return t('thread.starting.other.fsSnapshot');
    case 'fs.mkdir': return t('thread.starting.other.fsMkdir');
    case 'fs.write': return t('thread.starting.other.fsWrite');
    case 'env.attach': return t('thread.starting.other.envAttach');
    default: return t('thread.starting.other.work');
  }
}

/** The step a thread that starts is waiting for, in words. */
function startingStepWords(thread: Thread, t: TFunction, provider: string, agent: string): string {
  switch (thread.starting_step) {
    case 'environment':
      if (thread.environment_preparation !== null && thread.environment_preparation !== undefined && thread.environment_preparation.environment.id === thread.environment?.id) return t(`placeFacts.environmentStages.${thread.environment_preparation.kind}`);
      return t(thread.environment?.status === 'provisioning' ? 'thread.starting.environmentMaking' : 'thread.starting.environment');
    case 'source':
      return t('thread.starting.source');
    case 'recording':
      return t('thread.starting.recording');
    case 'files':
      return t('thread.starting.files');
    case 'provider':
      return t('thread.starting.provider', { provider });
    case 'harness':
      return t('thread.starting.harness', { agent });
    case 'credential':
      return t('thread.starting.credential', { provider });
    case 'session':
      return t('thread.starting.session');
    case 'turn':
      return t('thread.starting.turn', { agent });
    default:
      return t('thread.starting.none');
  }
}

/** When the step a starting thread waits for began (an instant, as the server wrote it), or null when it waits for nothing in
 * particular: the status line draws the time that has gone by since, beside the words of {@link waitingWords}. */
export function waitingSince(thread: Thread | null): string | null {
  if (thread?.state !== 'starting') return null;
  return thread.starting_since ?? null;
}

/** The words of a thread that is being stopped; null otherwise. */
export function stoppingWords(thread: Thread | null, t: TFunction, turn: Turn | null = null): string | null {
  return stoppingSince(thread, turn) === null ? null : t('thread.stopping.line');
}

/** A resource pause keeps its note apart from the fixed operational status. */
function pauseNote(raw: string, prefix: string): string | null {
  return raw.startsWith(`${prefix} · `) ? raw.slice(prefix.length + 3) : null;
}

/** The sentence for a thread that starts, is blocked or is held; `null` when it waits for nothing. */
export function waitingWords(thread: Thread | null, t: TFunction): string | null {
  if (thread === null) return null;
  if (thread.lock !== null) return t('thread.lock.line', { what: lockWords(thread.lock.kind, t), percent: thread.lock.percent });
  const provider = thread.binding?.provider.name ?? '';
  const agent = thread.binding?.agent.name ?? '';
  if (thread.state === 'starting') {
    const step = startingStepWords(thread, t, provider, agent);
    const work = thread.starting_work ?? null;
    const clone = thread.source?.state === 'preparing' ? thread.source.job?.clone ?? null : null;
    if (clone !== null && (work === null || work.command === 'git.prepare' && work.behind === null)) {
      return t('source.prepare.received', { count: clone.objects, bytes: formatBytes(clone.bytes, i18n.language), objects: formatNumber(clone.objects, i18n.language) });
    }
    if (work === null) return step;
    const what = startWork(work.command, t) ?? step;
    return work.behind === null ? what : t('thread.starting.behind', { what, other: otherWork(work.behind, t) });
  }
  if (thread.state === 'blocked' && thread.blocked !== null) {
    if (isAgentHistoryRemoving(thread)) return t('agentHistory.removing');
    // A held tool access says what the program itself reported, as the line before sending did.
    const readiness = thread.blocked.sandbox;
    const raw = readiness===null || readiness===undefined?thread.blocked.detail ?? '':[sandboxWords(readiness,t),readiness.sandbox.reason].filter((part)=>part!==null && part!=='').join(' ');
    const detail = raw === '' ? '' : t('thread.blocked.withDetail', { detail: blockedDetail(raw,t,thread.blocked.reason === 'source_failed' ? thread.source : null) });
    const common = { detail, provider, agent, model: thread.model_display ?? thread.model ?? '', machine: thread.environment?.name ?? '' };
    switch (thread.blocked.reason) {
      case 'unbound':
        // A removal takes the thread out of its environment and keeps its agent and provider.
        if (raw === ENVIRONMENT_REMOVED) return t('composer.gone.environmentUnnamed');
        return t('thread.blocked.unbound');
      case 'agent_offline':
        return t('thread.blocked.agent_offline', common);
      case 'agent_disabled':
        return t('thread.blocked.agent_disabled', common);
      case 'binding_invalid':
        // Held for the second step of the people who sent what waits: their setup is the way on.
        if (raw === 'second_step_required') return t('thread.blocked.secondStep');
        if (isMachineProgramVersionMismatch(raw)) return blockedDetail(raw, t);
        return confinementDetail(raw, t) ?? t('thread.blocked.binding_invalid', common);
      case 'provider_exhausted':
        return t('thread.blocked.provider_exhausted', common);
      case 'provider_unavailable': {
        if (isProviderRemoved(thread)) return t('resourceImpact.providerRemoved');
        if (thread.binding?.provider.status === 'disabled') {
          const note=pauseNote(raw,'disabled');
          return t('resourceImpact.providerDisabled',{name:provider,note:note===null?'':` · ${note}`});
        }
        return t('thread.blocked.provider_unavailable', common);
      }
      case 'environment_unavailable': {
        if (thread.environment?.status === 'disabled') {
          const note=pauseNote(raw,'disabled');
          return t('resourceImpact.environmentDisabled',{name:thread.environment.name,note:note===null?'':` · ${note}`});
        }
        const key = raw.startsWith('the environment failed: files:') ? 'startRecovery.filesFailed' : thread.environment?.status === 'failed' ? 'startRecovery.failed' : 'startRecovery.unavailable';
        const status = thread.environment?.status;
        const environment = thread.environment?.name ?? t('source.prepare.workspace');
        const why = key === 'startRecovery.failed' ? failedEnvironmentWhy(raw) : null;
        const reason = key === 'startRecovery.unavailable' && (status === 'offline' || status === 'removed' || status === 'moving' || status === 'recreating')
          ? t('startRecovery.lifecycle', { environment, state: t(`environments.status.${status}`) })
          : why !== null ? t('startRecovery.failedWhy', { environment, why }) : t(key, { environment });
        return thread.pending_inputs > 0 ? `${reason} ${t('startRecovery.held')}` : reason;
      }
      case 'model_not_set':
        return t('thread.blocked.model_not_set');
      case 'model_not_allowed': {
        const prefix=`the model ${thread.model ?? ''} is not enabled on the provider`;
        if(raw===prefix || raw.startsWith(`${prefix} · `)) {
          const note=pauseNote(raw,prefix);
          return t('resourceImpact.modelDisabled',{name:thread.model_display??thread.model??'',note:note===null?'':` · ${note}`});
        }
        return t('thread.blocked.model_not_allowed', common);
      }
      case 'quota_exceeded':
        return t('thread.blocked.quota_exceeded', common);
      case 'attachments':
        return t('thread.blocked.attachments');
      case 'workspace_trust':
        return t('thread.blocked.workspace_trust', common);
      case 'switching':
        return t('thread.blocked.switching');
      case 'source_failed':
        return t('source.prepare.stopped', { reason: sourceFailedWords(thread.source, t) ?? sourceFailure(t, 'workspace.source_failed') });
      case 'error':
        if (raw === NATIVE_END_UNCONFIRMED) return t('thread.blocked.nativeEnd');
        return t('thread.blocked.error', common);
    }
  }
  return null;
}

export function useWaitingWords(thread: Thread | null): string | null {
  const { t } = useTranslation();
  return waitingWords(thread, t);
}

export { stoppingSince } from '@/lib/threadStopping';
