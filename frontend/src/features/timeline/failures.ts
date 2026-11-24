/** One terminal failure owns repeated native errors and the actions for its unfinished task. */
import type { Item, Turn, TurnError } from '@/generated/api-types';
import type { ThreadView } from '@/realtime/stores/thread';

type FailureIdentity = Pick<TurnError, 'code' | 'message'> & { reason?: string | null; window?: string | null; resets_at?: string | null; details?: unknown };

function isHarnessNotice(error: FailureIdentity): boolean {
  return error.code === 'mcp_server_failed' || error.code === 'harness_diverged';
}

function reasonOf(error: FailureIdentity): string | null {
  if (typeof error.reason === 'string') return error.reason;
  const details = error.details;
  return typeof details === 'object' && details !== null && 'reason' in details && typeof details.reason === 'string' ? details.reason : null;
}

export function failureKey(error: FailureIdentity): string {
  if (error.code === 'image_generation_limit') return JSON.stringify([error.code, error.window ?? null, error.resets_at ?? null]);
  if (isHarnessNotice(error)) {
    // Different servers and settings are separate failures, even within the same turn.
    const details = typeof error.details === 'object' && error.details !== null && !Array.isArray(error.details)
      ? error.details as Record<string, unknown> : {};
    const named = (member: string): string | null => {
      const value = details[member];
      return typeof value === 'string' ? value : null;
    };
    return JSON.stringify([error.code, reasonOf(error), error.message, named('change'), named('server'), named('setting')]);
  }
  return JSON.stringify([error.code, reasonOf(error), error.code === 'harness_error' ? error.message : null]);
}

/** Native error rows may omit the terminal turn's more specific refusal reason. */
export function sameFailure(a: FailureIdentity, b: FailureIdentity): boolean {
  if (a.code === 'image_generation_limit' || b.code === 'image_generation_limit') return failureKey(a) === failureKey(b);
  if (isHarnessNotice(a) || isHarnessNotice(b)) return failureKey(a) === failureKey(b);
  const first = reasonOf(a), second = reasonOf(b);
  return a.code === b.code && (first === null || second === null || first === second)
    && (a.code !== 'harness_error' || a.message === b.message);
}

function automatic(view: ThreadView, turn: Turn): boolean {
  for (const item of view.items.values()) {
    if (item.kind === 'user_message' && item.payload.input_id === turn.input_id && item.payload.automatic !== null && item.payload.automatic !== undefined) return true;
    // A person's own Continue begins a turn of theirs, not another attempt of the rule's.
    if (item.kind === 'marker' && item.payload.marker === 'call_recovery' && item.payload.data.automatic && item.payload.data.input_id === turn.input_id) return true;
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

/**
 * What an agent program reported of its own failure, in a turn that failed because the program could not finish its work:
 * the same failure in the program's words, which the turn's one failure line carries below its sentence. A notice the
 * program raised beside it (a server that did not start, a setting it changed) stays a failure of its own.
 */
function programWords(turn: Turn | undefined, item: Item): boolean {
  return item.kind === 'error' && turn?.state === 'failed' && item.turn_id === turn.id
    && turn.error?.code === 'harness_error' && !isHarnessNotice(item.payload) && item.payload.code !== 'storage.data';
}

/** The program's own words for a failed turn, once each, in the order it said them. */
export function turnFailureWords(view: ThreadView, turn: Turn): string[] {
  const words: string[] = [];
  for (const id of view.order) {
    const item = view.items.get(id);
    if (item?.kind !== 'error' || !programWords(turn, item)) continue;
    const said = item.payload.message.trim();
    if (said !== '' && said !== turn.error?.message.trim() && !words.includes(said)) words.push(said);
  }
  return words;
}

/** Native duplicates stay in the history store; their terminal footer or last identical row renders them. */
export function hiddenFailureItem(view: ThreadView, item: Item): boolean {
  if (item.kind !== 'error' || item.turn_id === null) return false;
  const turn = view.turns.get(item.turn_id);
  if (turn?.state === 'failed' && turn.error !== null && sameFailure(turn.error, item.payload)) return true;
  if (programWords(turn, item)) return true;
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
