/** What fills a thread's context, what each of its turns used and what a turn was counted at, read from the turns the fake keeps, as the engine reads them from its rows. */
import { maybeHarnessName } from './harness-names.ts';
import type { ContextReports, ContextShortening, ThreadContext, ThreadTurnUsage, Turn, TurnCost } from '../../src/generated/api-types.ts';

/** What a harness of a kind reports, as its adapter declares it: Claude Code breaks its context down, Codex does not. */
export function reportsOf(harnessKind: string | null, turns: Iterable<Turn>): ContextReports {
  if (harnessKind === null) return { tokens: false, context: false, breakdown: false };
  let breakdown = harnessKind === 'claude-code';
  for (const turn of turns) if (turn.usage_detail !== null) breakdown = turn.usage_detail.breakdown;
  return { tokens: true, context: true, breakdown };
}

/** The turns, in their order. */
function ordered(turns: Iterable<Turn>): Turn[] {
  return [...turns].sort((a, b) => a.ordinal - b.ordinal);
}

/** The fake reports a name with the kind, as the API does. */
function namedHarness(kind: string | null) {
  if (kind === null) return null;
  const names: Record<string, string> = { codex: 'Codex', 'claude-code': 'Claude Code', fake: 'Example agent' };
  return { kind, name: names[kind] ?? 'Example agent', names: {} };
}

/** What fills the context, at the end of the latest turn that kept a fill, with the latest shortening. */
export function contextOf(turns: Iterable<Turn>, harnessKind: string | null): ThreadContext {
  const all = ordered(turns);
  const filled = [...all].reverse().find((turn) => turn.context !== null);
  let shortened: ContextShortening | null = null;
  for (const turn of all) {
    for (const shortening of turn.usage_detail?.shortenings ?? []) {
      if (shortened === null || shortening.at > shortened.at) shortened = shortening;
    }
  }
  const named = { harness: namedHarness(harnessKind) };
  return {
    ...named,
    harness_kind: harnessKind,
    harness: maybeHarnessName(harnessKind),
    reports: reportsOf(harnessKind, all),
    fill: filled?.context ?? null,
    turn_id: filled?.id ?? null,
    shortened,
    // The fake keeps no live session to count with.
    can_count: false,
  };
}

/** A page of the turns with what each used, the latest last: at most `limit`, numbered below `before`. */
export function turnUsagePage(turns: Iterable<Turn>, harnessKind: string | null, before: number | null, limit: number): ThreadTurnUsage {
  const all = ordered(turns);
  const below = all.filter((turn) => before === null || turn.ordinal < before);
  const page = below.slice(Math.max(0, below.length - limit));
  const named = { harness: namedHarness(harnessKind) };
  return {
    ...named,
    harness_kind: harnessKind,
    harness: maybeHarnessName(harnessKind),
    reports: reportsOf(harnessKind, all),
    turns: page.map((turn) => ({
      turn_id: turn.id,
      ordinal: turn.ordinal,
      started_at: turn.started_at,
      completed_at: turn.completed_at,
      model: turn.model,
      model_display: turn.model_display,
      usage: turn.usage,
      detail: turn.usage_detail ?? null,
    })),
    next_before: page.length < below.length ? (page[0]?.ordinal ?? null) : null,
  };
}

/** The fake's rate: a hundredth of a credit for a thousand tokens, as a catalogue's estimate, so that a price is shown. */
const CREDITS_PER_THOUSAND = 0.01;

/** What a turn was counted at: one row of its model, once it ended. */
export function turnCostOf(turn: Turn): TurnCost {
  const usage = turn.usage;
  if (usage === null || turn.completed_at === null) return { turn_id: turn.id, rows: [], credits: null, estimated_credits: '0', unrated_tokens: 0 };
  const credits = ((usage.total_tokens / 1000) * CREDITS_PER_THOUSAND).toFixed(4);
  return {
    turn_id: turn.id,
    rows: [{ model: turn.model, tokens: usage, credits, rate_source: 'catalogue' }],
    credits,
    estimated_credits: credits,
    unrated_tokens: 0,
  };
}
