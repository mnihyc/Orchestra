// @vitest-environment node
/** What fills a context and what a turn's tokens are made of: the tone of the ring at its thresholds, the bar's parts adding up, the parts of a turn with its sub-agents apart, and the export. */
import { describe, expect, it } from 'vitest';

import {
  cacheHitRate,
  contextTone,
  costParts,
  costSegments,
  csvCell,
  exportText,
  fillSegments,
  freedBy,
  mainOf,
  ringOf,
} from '@/features/usage/context/fill';
import type { ContextFill, ThreadUsage, TokenUsage, TurnUsageDetail, TurnUsageRow } from '@/generated/api-types';

const usage = (over: Partial<TokenUsage> = {}): TokenUsage => ({
  input_tokens: 10_000,
  cached_input_tokens: 6_000,
  cache_write_tokens: 1_000,
  output_tokens: 2_000,
  reasoning_tokens: 500,
  total_tokens: 12_000,
  ...over,
});

const thread = (used: number | null, window: number | null, compactAt: number | null): ThreadUsage => ({
  ...usage(),
  context_used: used,
  context_window: window,
  compact_at: compactAt,
  updated_at: '2026-10-14T10:00:00.000Z',
});

const detail = (over: Partial<TurnUsageDetail> = {}): TurnUsageDetail => ({
  subagents: null,
  by_subagent: [],
  models: [],
  context_window: 200_000,
  context_used: 142_000,
  compact_at: 167_000,
  shortenings: [],
  breakdown: false,
  ...over,
});

describe('the ring', () => {
  it('is green below 60 %, amber up to 85 % and red above', () => {
    expect(contextTone(59.9)).toBe('green');
    expect(contextTone(60)).toBe('amber');
    expect(contextTone(85)).toBe('amber');
    expect(contextTone(85.1)).toBe('red');
  });

  it('is the share of the window the last prompt filled, with the tick where the conversation is shortened', () => {
    const ring = ringOf(thread(142_000, 200_000, 180_000));
    expect(ring).toMatchObject({ used: 142_000, window: 200_000, tone: 'amber' });
    expect(ring?.percent).toBeCloseTo(71);
    expect(ring?.compactPercent).toBeCloseTo(90);
  });

  it('is none where the harness reported no size or no window, and has no tick where it said nothing of shortening', () => {
    expect(ringOf(null)).toBeNull();
    expect(ringOf(thread(null, 200_000, null))).toBeNull();
    expect(ringOf(thread(1_000, null, null))).toBeNull();
    expect(ringOf(thread(1_000, 200_000, null))?.compactPercent).toBeNull();
  });
});

describe('the bar of a context', () => {
  const fill: ContextFill = {
    count: 'orch',
    used: 100_000,
    window: 200_000,
    compact_at: 180_000,
    scaled: false,
    at: '2026-10-14T10:00:00.000Z',
    parts: [
      { kind: 'harness', tokens: 30_000, count: 'orch', rows: [] },
      { kind: 'tool_calls', tokens: 70_000, count: 'orch', rows: [] },
    ],
  };

  it('has the parts, then free space, then the reserve kept for shortening, and they make the whole window', () => {
    const segments = fillSegments(fill);
    expect(segments.map((segment) => segment.key)).toEqual(['harness', 'tool_calls', 'free', 'reserve']);
    expect(segments.map((segment) => segment.tokens)).toEqual([30_000, 70_000, 80_000, 20_000]);
    expect(segments.reduce((sum, segment) => sum + segment.share, 0)).toBeCloseTo(1);
  });

  it('is measured against what fills it where no window is known, and gives every part a look', () => {
    const segments = fillSegments({ ...fill, window: null, compact_at: null });
    expect(segments.map((segment) => segment.key)).toEqual(['harness', 'tool_calls']);
    expect(segments[1]?.share).toBeCloseTo(0.7);
    expect(segments.every((segment) => segment.look.pattern.trim().length > 0)).toBe(true);
  });
});

describe('the parts of a turn', () => {
  it('are fresh input, cache read, cache write, output and reasoning, adding up to the turn', () => {
    const parts = costParts(usage(), null);
    expect(parts).toEqual([
      { part: 'fresh', tokens: 3_000 },
      { part: 'cache_read', tokens: 6_000 },
      { part: 'cache_write', tokens: 1_000 },
      { part: 'output', tokens: 1_500 },
      { part: 'reasoning', tokens: 500 },
    ]);
    expect(parts.reduce((sum, part) => sum + part.tokens, 0)).toBe(12_000);
  });

  it('keep the sub-agents apart, out of the main conversation, so that the bar still adds up to the turn', () => {
    const subagents = usage({ input_tokens: 2_000, cached_input_tokens: 1_000, cache_write_tokens: 0, output_tokens: 1_000, reasoning_tokens: 0, total_tokens: 3_000 });
    const withSubagents = detail({ subagents });
    expect(mainOf(usage(), withSubagents)).toMatchObject({ input_tokens: 8_000, output_tokens: 1_000, total_tokens: 9_000 });
    const parts = costParts(usage(), withSubagents);
    expect(parts.at(-1)).toEqual({ part: 'subagents', tokens: 3_000 });
    expect(parts.reduce((sum, part) => sum + part.tokens, 0)).toBe(12_000);
    expect(costSegments(usage(), withSubagents).reduce((sum, segment) => sum + segment.share, 0)).toBeCloseTo(1);
  });

  it('leave out what the harness did not report, and say the cache hit rate of the input', () => {
    expect(costParts(usage({ cache_write_tokens: 0, reasoning_tokens: 0 }), null).map((part) => part.part)).toEqual(['fresh', 'cache_read', 'output']);
    expect(cacheHitRate(usage())).toBeCloseTo(0.6);
    expect(cacheHitRate(usage({ input_tokens: 0, cached_input_tokens: 0 }))).toBeNull();
  });
});

describe('a shortening and the export', () => {
  it('says what a shortening freed where both sizes are known', () => {
    expect(freedBy({ before: 160_000, after: 30_000 })).toBe(130_000);
    expect(freedBy({ before: 160_000, after: null })).toBeNull();
    expect(freedBy(null)).toBeNull();
  });

  it('writes a row for each turn under its header, quoting what would break a line', () => {
    const row: TurnUsageRow = {
      turn_id: 't1',
      ordinal: 1,
      started_at: '2026-10-14T10:00:00.000Z',
      completed_at: '2026-10-14T10:01:00.000Z',
      model: 'gpt-6-sol',
      model_display: 'GPT-6 Sol',
      usage: usage(),
      detail: detail({ shortenings: [{ at: '2026-10-14T10:00:30.000Z', before: 150_000, after: 40_000, automatic: true }] }),
    };
    const lines = exportText([row]).trim().split('\n');
    expect(lines[0]).toBe('turn,started_at,model,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,subagent_tokens,total_tokens,cache_hit_rate,context_used,context_window,shortened');
    expect(lines[1]).toBe('1,2026-10-14T10:00:00.000Z,gpt-6-sol,10000,6000,1000,2000,500,,12000,0.6,142000,200000,1');
    expect(csvCell('a, "b"')).toBe('"a, ""b"""');
  });
});
