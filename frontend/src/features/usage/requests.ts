/** Pause and resume of the live request log, with stable rows while arrivals wait. */
import type { UsageEvent } from '@/generated/api-types';
import type { UsageFilters } from '@/api/usage';

export interface RequestLog {
  paused: boolean;
  rows: ReadonlyMap<string, UsageEvent>;
  waiting: ReadonlyMap<string, UsageEvent>;
}
export const emptyLog = (): RequestLog => ({ paused: false, rows: new Map(), waiting: new Map() });
export type LogAction = { type: 'live' | 'history'; rows: readonly UsageEvent[] } | { type: 'pause' } | { type: 'resume' } | { type: 'reset' };

export function requestLog(state: RequestLog, action: LogAction): RequestLog {
  if (action.type === 'reset') return emptyLog();
  if (action.type === 'pause') return { ...state, paused: true };
  if (action.type === 'resume') return { paused: false, rows: new Map([...state.rows, ...state.waiting]), waiting: new Map() };
  if (state.paused && action.type === 'history') {
    // A focus-triggered HTTP refresh is still an arrival. Explicit older pages
    // may extend the bottom, but a refreshed first page cannot jump above the reader.
    const oldest = [...state.rows.values()].reduce<string | null>((at, row) => at === null || row.at < at ? row.at : at, null);
    let visible = state.rows;
    const waiting = new Map(state.waiting);
    for (const row of action.rows) {
      const existing = waiting.get(row.id) ?? state.rows.get(row.id);
      if (existing !== undefined && !existing.running && row.running) continue;
      if (oldest !== null && row.at < oldest && !state.rows.has(row.id)) {
        if (visible === state.rows) visible = new Map(state.rows);
        (visible as Map<string, UsageEvent>).set(row.id, row);
      } else waiting.set(row.id, row);
    }
    return { ...state, rows: visible, waiting };
  }
  const queue = action.type === 'live' && state.paused;
  const rows = new Map(queue ? state.waiting : state.rows);
  for (const row of action.rows) {
    const existing = rows.get(row.id) ?? state.rows.get(row.id);
    // A delayed running snapshot must never turn a completed record back into a running one.
    if (existing !== undefined && !existing.running && row.running) continue;
    rows.set(row.id, row);
  }
  return queue ? { ...state, waiting: rows } : { ...state, rows };
}

/** The server enforces scope; these are only the reader's optional display filters. */
export function matchesRequest(row: UsageEvent, filters: UsageFilters): boolean {
  const same = (asked: string | undefined, value: string | null | undefined) => asked === undefined || (asked === 'none' ? value === null || value === undefined : asked === value);
  const outcome = row.running ? 'running' : row.timing?.outcome.state === 'told' ? row.timing.outcome.value : 'not_measured';
  return same(filters.user_id, row.user?.id) && same(filters.model, row.model) && same(filters.provider_id, row.provider.id)
    && same(filters.harness_kind, row.harness_kind) && same(filters.environment_kind, row.environment_kind)
    && same(filters.project_id, row.project.id) && same(filters.topic_id, row.topic.id) && same(filters.thread_id, row.thread?.id)
    && same(filters.effort, row.effort) && same(filters.tier, row.tier) && same(filters.program, row.access_program)
    && (filters.outcome === 'errors' ? outcome !== 'done' && outcome !== 'forced' && outcome !== 'running' && outcome !== 'not_measured' : filters.outcome === 'not_measured' ? row.timing?.outcome.state !== 'told' : same(filters.outcome, outcome))
    && same(filters.turn_id, row.turn_id);
}

export function newestRequests(rows: ReadonlyMap<string, UsageEvent>): UsageEvent[] {
  return [...rows.values()].sort((a, b) => b.at.localeCompare(a.at) || b.id.localeCompare(a.id));
}
