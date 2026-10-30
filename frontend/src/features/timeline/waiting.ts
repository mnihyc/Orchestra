/** What a thread is waiting for, in words: the step of its start, the reason it is blocked, the job that holds it.
 *
 * The same words stand under the input that waits, in the state chip of the header and in the status
 * line that ends the conversation, so they are made once, here.
 */
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';

import type { Thread } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { sandboxWords } from '@/features/resources/SandboxReadiness';

const CONFINEMENT_KEYS=['threadConfinement.approvalNotAllowed','threadConfinement.approvalUnsupported','threadConfinement.sandboxUnsupported','agentHistory.removingDetail','agentHistory.sideNeedsImport'] as const;

/** This automatic native-cache wait has no human recovery action and is not an error. */
export function isAgentHistoryRemoving(thread: Thread): boolean {
  return thread.state === 'blocked' && thread.blocked?.detail === i18n.getFixedT('en')('agentHistory.removingDetail');
}

/** Only these fixed engine explanations are translated; unknown/native error details remain verbatim. */
export function blockedDetail(raw:string,t:TFunction):string {
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
    switch (thread.starting_step) {
      case 'environment':
        return t('thread.starting.environment');
      case 'source':
        return t('thread.starting.source');
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
  if (thread.state === 'blocked' && thread.blocked !== null) {
    if (isAgentHistoryRemoving(thread)) return t('agentHistory.removing');
    const raw = thread.blocked.sandbox===null || thread.blocked.sandbox===undefined?thread.blocked.detail ?? '':sandboxWords(thread.blocked.sandbox,t);
    const detail = raw === '' ? '' : t('thread.blocked.withDetail', { detail: blockedDetail(raw,t) });
    const common = { detail, provider, agent, model: thread.model_display ?? thread.model ?? '', machine: thread.environment?.name ?? '' };
    switch (thread.blocked.reason) {
      case 'unbound':
        return t('thread.blocked.unbound');
      case 'agent_offline':
        return t('thread.blocked.agent_offline', common);
      case 'agent_disabled':
        return t('thread.blocked.agent_disabled', common);
      case 'binding_invalid':
        // Held for the second step of the people who sent what waits: their setup is the way on.
        if (raw === 'second_step_required') return t('thread.blocked.secondStep');
        return t('thread.blocked.binding_invalid', common);
      case 'provider_exhausted':
        return t('thread.blocked.provider_exhausted', common);
      case 'provider_unavailable': {
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
        const reason = t(key, { environment: thread.environment?.name ?? t('source.prepare.workspace') });
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
      case 'error':
        return t('thread.blocked.error', common);
    }
  }
  return null;
}

export function useWaitingWords(thread: Thread | null): string | null {
  const { t } = useTranslation();
  return waitingWords(thread, t);
}
