/** One terminal failure owns repeated native errors and the actions for its unfinished task. */
import type { Item, Turn, TurnError } from '@/generated/api-types';
import type { ThreadView } from '@/realtime/stores/thread';

export function failureKey(error: Pick<TurnError, 'code' | 'message'> & { reason?: string | null }): string {
  return JSON.stringify([error.code, error.reason ?? null, error.code === 'harness_error' ? error.message : null]);
}

/** Native error rows may omit the terminal turn's more specific refusal reason. */
export function sameFailure(a: Pick<TurnError, 'code' | 'message'> & { reason?: string | null }, b: Pick<TurnError, 'code' | 'message'> & { reason?: string | null }): boolean {
  return a.code === b.code && (a.reason === null || a.reason === undefined || b.reason === null || b.reason === undefined || a.reason === b.reason)
    && (a.code !== 'harness_error' || a.message === b.message);
}

function automatic(view: ThreadView, turn: Turn): boolean {
  for (const item of view.items.values()) {
    if (item.kind === 'user_message' && item.payload.input_id === turn.input_id && item.payload.automatic !== null && item.payload.automatic !== undefined) return true;
    if (item.kind === 'marker' && item.payload.marker === 'call_recovery' && item.payload.data.input_id === turn.input_id) return true;
  }
  return false;
}

/** Repeated attempts in one recovery chain retain a single terminal error, with their real count. */
export function failureGroup(view: ThreadView, turn: Turn): { owner: string; attempts: number } {
  const turns = [...view.turns.values()].sort((a, b) => a.ordinal - b.ordinal);
  const index = turns.findIndex((held) => held.id === turn.id);
  const joins = (before: Turn | undefined, after: Turn | undefined) => before?.state === 'failed' && after?.state === 'failed'
    && before.error !== null && after.error !== null && failureKey(before.error) === failureKey(after.error)
    && (before.input_id === after.input_id || automatic(view, after));
  let first = index, last = index;
  while (first > 0 && joins(turns[first - 1], turns[first])) first--;
  while (last >= 0 && last + 1 < turns.length && joins(turns[last], turns[last + 1])) last++;
  let attempts = 0;
  for (const held of turns.slice(first, last + 1)) {
    const errors = [...view.items.values()].filter((item) => item.kind === 'error' && item.turn_id === held.id
      && held.error !== null && sameFailure(item.payload, held.error));
    attempts += Math.max(1, errors.length);
  }
  return { owner: turns[last]?.id ?? turn.id, attempts: Math.max(1, attempts) };
}

/** Native duplicates stay in the history store; their terminal footer or last identical row renders them. */
export function hiddenFailureItem(view: ThreadView, item: Item): boolean {
  if (item.kind !== 'error' || item.turn_id === null) return false;
  const turn = view.turns.get(item.turn_id);
  if (turn?.state === 'failed' && turn.error !== null && sameFailure(turn.error, item.payload)) return true;
  for (let at = view.order.length - 1; at >= 0; at--) {
    const other = view.items.get(view.order[at] ?? '');
    if (other?.kind === 'error' && other.turn_id === item.turn_id && failureKey(other.payload) === failureKey(item.payload)) return other.id !== item.id;
  }
  return false;
}

/** A later turn or active recovery owns the next action; older turns keep only their recorded facts. */
export function manualFailureActions(view: ThreadView, turn: Turn): boolean {
  const current = view.thread?.current_turn_id;
  return !view.hasNewer && view.thread?.blocked?.detail !== 'call_recovery'
    && (current === null || current === undefined || current === turn.id)
    && ![...view.turns.values()].some((other) => other.ordinal > turn.ordinal);
}
