/** Active output totals change with admitted rows and stream pieces, independently of held history. */
import type { Item } from '@/generated/api-types';
import { countTextCharacters, type CharacterCounts } from '@/lib/tokenEstimate';

// The server's inline-input budget, conservatively measured in UTF-16 units here. Larger snapshots
// await reported usage instead of making a status render scan an arbitrary body on the main thread.
export const OUTPUT_COUNT_SNAPSHOT_CHARS = 100_000;

export interface OutputCounts {
  turnId: string | null;
  generation: object;
  counts: CharacterCounts | null;
}

interface Observation { generations: WeakSet<object>; counts: CharacterCounts | null; lastCodeUnit: number }
// Weak keys retain neither previous message bodies nor a second history of item identities.
const observations = new WeakMap<Item, Observation>();

export function newOutputCounts(turnId: string | null): OutputCounts {
  return { turnId, generation: {}, counts: { ascii: 0, other: 0 } };
}

/** A delta's new suffix is counted once; snapshots replace their previous contribution. */
export function observeOutput(state: OutputCounts, before: Item | undefined, after: Item, appended?: string): OutputCounts {
  if (state.turnId === null || after.kind !== 'agent_message' || after.stream !== 'main' || after.turn_id !== state.turnId) return state;
  const previous = before === undefined ? undefined : observations.get(before);
  let counts: CharacterCounts | null;
  let lastCodeUnit = Number.NaN;
  if (appended !== undefined && before?.kind === 'agent_message' && previous !== undefined) {
    lastCodeUnit = appended.length === 0 ? previous.lastCodeUnit : appended.charCodeAt(appended.length - 1);
    if (previous.counts === null) counts = null;
    else {
      const suffix = countTextCharacters(appended);
      // Code-point offsets normally keep pairs together; preserve one count for a split pair too.
      const end = previous.lastCodeUnit;
      const start = appended.charCodeAt(0);
      const paired = end >= 0xd800 && end <= 0xdbff && start >= 0xdc00 && start <= 0xdfff ? 1 : 0;
      counts = { ascii: previous.counts.ascii + suffix.ascii, other: previous.counts.other + suffix.other - paired };
    }
  } else {
    counts = after.payload.text.length > OUTPUT_COUNT_SNAPSHOT_CHARS ? null : countTextCharacters(after.payload.text);
    if (counts !== null) lastCodeUnit = after.payload.text.charCodeAt(after.payload.text.length - 1);
  }
  const removed = previous?.generations.has(state.generation) === true ? previous.counts : { ascii: 0, other: 0 };
  // Several open projections can admit the same row object. Membership belongs to each view,
  // and weak generation keys let a closed view disappear with its ordinary store entry.
  const generations = observations.get(after)?.generations ?? new WeakSet<object>();
  generations.add(state.generation);
  observations.set(after, { generations, counts, lastCodeUnit });
  return { ...state, counts: state.counts === null || counts === null || removed === null ? null : {
    ascii: state.counts.ascii + counts.ascii - removed.ascii,
    other: state.counts.other + counts.other - removed.other,
  } };
}
