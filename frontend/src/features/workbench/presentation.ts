/** Recoverable presentation only: IDs, expansion choices and scroll offsets, never resource data or authority. */
export type Presentations = Record<string, Record<string, unknown>>;
let held = new Map<string, Map<string, unknown>>();

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The signed-in workspace owns these snapshots. Consumers validate their own versioned value. */
export function restorePresentations(value: unknown): void {
  held = new Map();
  if (!record(value)) return;
  for (const [owner, entries] of Object.entries(value)) if (record(entries)) held.set(owner, new Map(Object.entries(entries)));
}

export function readPresentation(owner: string, key: string): unknown { return held.get(owner)?.get(key); }

export function keepPresentation(owner: string, key: string, value: unknown): void {
  let entries = held.get(owner);
  if (entries === undefined) { entries = new Map(); held.set(owner, entries); }
  entries.set(key, value);
}

/** Only tabs still open or available through Reopen belong to the saved workspace. */
export function capturePresentations(owners: ReadonlySet<string>): Presentations {
  return Object.fromEntries([...held].filter(([owner]) => owners.has(owner)).map(([owner, entries]) => [owner, Object.fromEntries(entries)]));
}
