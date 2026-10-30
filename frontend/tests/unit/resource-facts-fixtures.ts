/** Applicable cap observations used by compact resource hints and card tests. */
import type { CapStatus } from '@/generated/api-types';
import { planProvider } from './provider-plan-fixtures';

export function choiceCap(patch: Partial<CapStatus> = {}): CapStatus {
  const person = planProvider().owner;
  return { cap: { id: 'cap', node: null, provider: null, subject: { kind: 'everyone' }, split: 'shared', models: [], tiers: [], harness_kinds: [], meter: 'running', period: null, counted_from: null, amount: '5', at_limit: 'finish_turn', warn_percent: null, uncounted: 'allow', note: '', enabled: true, revision: 1, created_by: person, created_at: '2026-10-25T12:00:00Z', updated_at: '2026-10-25T12:00:00Z', can_edit: false },
    person: null, used: '3', in_progress: '0', waiting: 0, left_out: { unrated_tokens: 0, unmetered_threads: 0 }, state: 'ok', period_ends_at: null, replaced_by: null, can_bypass: false, bypasses: { count: 0, last: null }, ...patch };
}
