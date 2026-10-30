/** What fills a context and what a turn's tokens are made of, as parts of a bar: shares, tones, colours with patterns, and the words of each.
 *
 * The ring in the thread's header says how full the context is at a glance: the share of the window the
 * last prompt filled, green below 60 %, amber up to 85 %, red above, with a tick where the harness shortens
 * the conversation. The bar of the context panel says what fills it, part by part, then what is free and
 * what the harness keeps free for shortening. The bar of a turn says what its tokens were: fresh input,
 * read from the cache, written to the cache, output and its reasoning, and the sub-agents' share. Every part
 * has a colour and a pattern of its own, so that none is told apart by colour alone, and its numbers are
 * written beside it wherever it is drawn.
 */

import type { ContextCount, ContextFill, ContextPartKind, Thread, TokenUsage, TurnUsageDetail, TurnUsageRow } from '@/generated/api-types';

/** From this share of the window the ring is amber: the conversation will be shortened before long. */
export const CONTEXT_AMBER_FROM = 60;
/** Above this share it is red: the harness is about to shorten the conversation, or is past its point. */
export const CONTEXT_RED_ABOVE = 85;

export type ContextTone = 'green' | 'amber' | 'red';

/** The tone of a ring at `percent` of the window. */
export function contextTone(percent: number): ContextTone {
  return percent > CONTEXT_RED_ABOVE ? 'red' : percent >= CONTEXT_AMBER_FROM ? 'amber' : 'green';
}

/** How full a thread's context is, as its harness last reported it; `null` where it reported no size or no window. */
export interface Ring {
  used: number;
  window: number;
  /** The share of the window, 0 to 100 and beyond where the harness went past it. */
  percent: number;
  /** Where the conversation is shortened, as a share of the window; `null` where that is not known. */
  compactPercent: number | null;
  tone: ContextTone;
}

export function ringOf(usage: Thread['usage']): Ring | null {
  if (usage === null) return null;
  const used = usage.context_used ?? null;
  const window = usage.context_window ?? null;
  if (used === null || window === null || window <= 0) return null;
  const percent = (used / window) * 100;
  const compactAt = usage.compact_at ?? null;
  return {
    used,
    window,
    percent,
    compactPercent: compactAt === null || compactAt <= 0 ? null : Math.min(100, (compactAt / window) * 100),
    tone: contextTone(percent),
  };
}

/** A pattern drawn over a colour, so that two parts never differ by colour alone. */
export type Pattern = 'solid' | 'stripes' | 'dots' | 'cross' | 'diagonal' | 'grid';

/** How a part is drawn: one of the chart's colours (or the quiet one), and a pattern. */
export interface Look {
  colour: string;
  pattern: Pattern;
}

/** The look of each part of a context: the harness's own quiet and hatched, Orch's in blues and violets, the conversation warm. */
export const PART_LOOKS: Record<ContextPartKind, Look> = {
  harness: { colour: 'var(--text-subtle)', pattern: 'diagonal' },
  orch_instructions: { colour: 'var(--chart-1)', pattern: 'solid' },
  orch_tools: { colour: 'var(--chart-7)', pattern: 'stripes' },
  mcp_tools: { colour: 'var(--chart-5)', pattern: 'dots' },
  memory_files: { colour: 'var(--chart-6)', pattern: 'grid' },
  skills_agents: { colour: 'var(--chart-4)', pattern: 'cross' },
  people_messages: { colour: 'var(--chart-3)', pattern: 'solid' },
  agent_messages: { colour: 'var(--chart-2)', pattern: 'stripes' },
  tool_calls: { colour: 'var(--chart-8)', pattern: 'dots' },
  files: { colour: 'var(--chart-4)', pattern: 'diagonal' },
  other: { colour: 'var(--text-subtle)', pattern: 'dots' },
};

/** Free space: nothing drawn but its outline. */
export const FREE_LOOK: Look = { colour: 'transparent', pattern: 'solid' };
/** What the harness keeps free for shortening: the quiet colour in a grid. */
export const RESERVE_LOOK: Look = { colour: 'var(--border)', pattern: 'grid' };

/** The CSS of a look: its colour, and its pattern as gradients of a lighter tone over it. */
export function lookStyle(look: Look): { backgroundColor: string; backgroundImage?: string; backgroundSize?: string } {
  const light = 'rgb(255 255 255 / 0.45)';
  switch (look.pattern) {
    case 'solid':
      return { backgroundColor: look.colour };
    case 'stripes':
      return { backgroundColor: look.colour, backgroundImage: `repeating-linear-gradient(90deg, ${light} 0 1px, transparent 1px 4px)` };
    case 'diagonal':
      return { backgroundColor: look.colour, backgroundImage: `repeating-linear-gradient(45deg, ${light} 0 1.5px, transparent 1.5px 5px)` };
    case 'dots':
      return { backgroundColor: look.colour, backgroundImage: `radial-gradient(${light} 1px, transparent 1.5px)`, backgroundSize: '4px 4px' };
    case 'cross':
      return {
        backgroundColor: look.colour,
        backgroundImage: `repeating-linear-gradient(45deg, ${light} 0 1px, transparent 1px 5px), repeating-linear-gradient(-45deg, ${light} 0 1px, transparent 1px 5px)`,
      };
    case 'grid':
      return {
        backgroundColor: look.colour,
        backgroundImage: `linear-gradient(${light} 1px, transparent 1px), linear-gradient(90deg, ${light} 1px, transparent 1px)`,
        backgroundSize: '4px 4px',
      };
  }
}

/** One part of a bar: what it is, its tokens, its share of the whole bar, and how it was counted. */
export interface Segment<Key extends string = string> {
  key: Key;
  tokens: number;
  /** Its share of the bar, 0 to 1. */
  share: number;
  look: Look;
  /** For a part of a context: how it was counted. */
  count?: ContextCount;
}

/** What a part of the bar of a context is: a part of the context, free space, or the reserve. */
export type FillKey = ContextPartKind | 'free' | 'reserve';

/** The bar of a context: its parts, then free space and the reserve the harness keeps for shortening, each a share of the window (of what fills it where no window is known). */
export function fillSegments(fill: ContextFill): Segment<FillKey>[] {
  const window = fill.window ?? null;
  const whole = window !== null && window >= fill.used ? window : fill.used;
  if (whole <= 0) return [];
  const segments: Segment<FillKey>[] = fill.parts
    .filter((part) => part.tokens > 0)
    .map((part) => ({ key: part.kind, tokens: part.tokens, share: part.tokens / whole, look: PART_LOOKS[part.kind], count: part.count }));
  if (window !== null && window > fill.used) {
    const compactAt = fill.compact_at ?? null;
    const reserve = compactAt !== null && compactAt > fill.used && compactAt < window ? window - compactAt : 0;
    const free = window - fill.used - reserve;
    if (free > 0) segments.push({ key: 'free', tokens: free, share: free / whole, look: FREE_LOOK });
    if (reserve > 0) segments.push({ key: 'reserve', tokens: reserve, share: reserve / whole, look: RESERVE_LOOK });
  }
  return segments;
}

/** The parts of what a turn used: fresh input, read from the cache, written to the cache, output, reasoning, and its sub-agents. */
export type CostPart = 'fresh' | 'cache_read' | 'cache_write' | 'output' | 'reasoning' | 'subagents';

export const COST_LOOKS: Record<CostPart, Look> = {
  fresh: { colour: 'var(--chart-1)', pattern: 'solid' },
  cache_read: { colour: 'var(--chart-3)', pattern: 'stripes' },
  cache_write: { colour: 'var(--chart-4)', pattern: 'dots' },
  output: { colour: 'var(--chart-2)', pattern: 'solid' },
  reasoning: { colour: 'var(--chart-7)', pattern: 'diagonal' },
  subagents: { colour: 'var(--chart-5)', pattern: 'cross' },
};

const ZERO: TokenUsage = { input_tokens: 0, cached_input_tokens: 0, cache_write_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0 };

/** What a turn's own conversation used: its totals less its sub-agents' share, part by part and never below nothing. */
export function mainOf(usage: TokenUsage, detail: TurnUsageDetail | null | undefined): TokenUsage {
  const subagents = detail?.subagents ?? ZERO;
  const less = (a: number, b: number) => Math.max(0, a - b);
  const input = less(usage.input_tokens, subagents.input_tokens);
  const output = less(usage.output_tokens, subagents.output_tokens);
  return {
    input_tokens: input,
    cached_input_tokens: Math.min(input, less(usage.cached_input_tokens, subagents.cached_input_tokens)),
    cache_write_tokens: Math.min(input, less(usage.cache_write_tokens ?? 0, subagents.cache_write_tokens ?? 0)),
    output_tokens: output,
    reasoning_tokens: Math.min(output, less(usage.reasoning_tokens, subagents.reasoning_tokens)),
    total_tokens: input + output,
  };
}

/** How many tokens each part of a turn's use is, in the order of its bar. Parts of nothing are left out. */
export function costParts(usage: TokenUsage, detail: TurnUsageDetail | null | undefined): { part: CostPart; tokens: number }[] {
  const main = mainOf(usage, detail);
  const read = main.cached_input_tokens;
  const written = Math.min(main.cache_write_tokens ?? 0, main.input_tokens - read);
  const parts: { part: CostPart; tokens: number }[] = [
    { part: 'fresh', tokens: Math.max(0, main.input_tokens - read - written) },
    { part: 'cache_read', tokens: read },
    { part: 'cache_write', tokens: written },
    { part: 'output', tokens: Math.max(0, main.output_tokens - main.reasoning_tokens) },
    { part: 'reasoning', tokens: main.reasoning_tokens },
    { part: 'subagents', tokens: detail?.subagents?.total_tokens ?? 0 },
  ];
  return parts.filter((part) => part.tokens > 0);
}

/** The bar of a turn: its parts as shares of what they add up to. */
export function costSegments(usage: TokenUsage, detail: TurnUsageDetail | null | undefined): Segment<CostPart>[] {
  const parts = costParts(usage, detail);
  const whole = parts.reduce((sum, part) => sum + part.tokens, 0);
  if (whole <= 0) return [];
  return parts.map((part) => ({ key: part.part, tokens: part.tokens, share: part.tokens / whole, look: COST_LOOKS[part.part] }));
}

/** The share of the input that was read from the cache, 0 to 1; `null` where there was no input. */
export function cacheHitRate(usage: TokenUsage): number | null {
  return usage.input_tokens > 0 ? Math.min(1, usage.cached_input_tokens / usage.input_tokens) : null;
}

/** The catalogue key of how a part was counted. */
export function countKey(count: ContextCount): 'contextUsage.count.counted' | 'contextUsage.count.estimated' | 'contextUsage.count.orch' {
  return count === 'counted' ? 'contextUsage.count.counted' : count === 'estimated' ? 'contextUsage.count.estimated' : 'contextUsage.count.orch';
}

/** How much a shortening freed, where both sizes are known. */
export function freedBy(shortening: { before?: number | null; after?: number | null } | null | undefined): number | null {
  if (shortening === null || shortening === undefined) return null;
  const before = shortening.before ?? null;
  const after = shortening.after ?? null;
  return before !== null && after !== null && before > after ? before - after : null;
}

/** The cells of one turn's row of the export, in the order of its header. */
export const EXPORT_HEADER = ['turn', 'started_at', 'model', 'input_tokens', 'cached_input_tokens', 'cache_write_tokens', 'output_tokens', 'reasoning_tokens', 'subagent_tokens', 'total_tokens', 'cache_hit_rate', 'context_used', 'context_window', 'shortened'] as const;

/** A CSV cell: quoted where it holds what would break the line. */
export function csvCell(value: string | number | null): string {
  if (value === null) return '';
  const text = String(value);
  return /[",\n\r]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

/** A turn's share of its window at its end, 0 to 100; `null` where its harness reported no size or no window. */
export function fillPercent(row: TurnUsageRow): number | null {
  const used = row.detail?.context_used ?? null;
  const window = row.detail?.context_window ?? null;
  return used === null || window === null || window <= 0 ? null : Math.min(100, (used / window) * 100);
}

/** The rows of the export of a thread's turns, under their header. */
export function exportText(rows: readonly TurnUsageRow[]): string {
  const lines = [EXPORT_HEADER.join(',')];
  for (const row of rows) {
    const usage = row.usage ?? null;
    const detail = row.detail ?? null;
    const hit = usage === null ? null : cacheHitRate(usage);
    lines.push(
      [
        row.ordinal,
        row.started_at ?? null,
        row.model,
        usage?.input_tokens ?? null,
        usage?.cached_input_tokens ?? null,
        usage === null ? null : (usage.cache_write_tokens ?? 0),
        usage?.output_tokens ?? null,
        usage?.reasoning_tokens ?? null,
        detail?.subagents?.total_tokens ?? null,
        usage?.total_tokens ?? null,
        hit === null ? null : Number(hit.toFixed(4)),
        detail?.context_used ?? null,
        detail?.context_window ?? null,
        detail === null ? null : detail.shortenings.length,
      ]
        .map(csvCell)
        .join(','),
    );
  }
  return `${lines.join('\n')}\n`;
}
