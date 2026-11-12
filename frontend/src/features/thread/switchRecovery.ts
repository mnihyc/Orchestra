/** The existing contextual recovery owns one visible Switch operation above the composer. */
import type { Thread } from '@/generated/api-types';
import { isAgentHistoryRemoving } from '@/features/timeline/waiting';
import { holds } from '@/lib/permissions';

const SWITCH_WAYS: Readonly<Record<string, 'switch' | 'switchProfile' | 'switchProvider'>> = {
  agent_offline: 'switch',
  agent_disabled: 'switchProfile',
  binding_invalid: 'switchProfile',
  provider_exhausted: 'switchProvider',
  provider_unavailable: 'switchProvider',
  model_not_allowed: 'switchProvider',
};

/** A historical view has no recovery banner and retains the header's existing action state. */
export function threadSwitchRecovery(thread: Thread, readOnly = false) {
  const blocked = thread.state === 'blocked' && thread.blocked !== null && thread.lock === null ? thread.blocked : null;
  if (readOnly || blocked === null || (blocked.reason === 'binding_invalid' && blocked.detail === 'second_step_required') || blocked.detail === 'call_recovery' ||
    isAgentHistoryRemoving(thread) || !holds(thread.access, 'thread.manage')) return undefined;
  return SWITCH_WAYS[blocked.reason];
}
