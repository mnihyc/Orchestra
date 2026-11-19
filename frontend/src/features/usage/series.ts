/** What the chart is drawn from: the series of an answer laid out as bars, one stack for each member of the dimension it is split by, and the colour each stack keeps.
 *
 * Pure functions, so that "the bars add up to the table" can be tested without drawing a bar. A stack is
 * a member of a dimension (a model, a person); the key that stands for all the members beyond the eight
 * largest is the last stack and is drawn in a quiet colour of its own.
 */
import type { TFunction } from 'i18next';

import type { UsageAnalytics, UsageBucket, UsageDimension } from '@/generated/api-types';
import { creditsState, measureOf } from '@/features/usage/figures';
import { keyDetail, keyId, keyLabel, isRest } from '@/features/usage/keys';
import type { Measure, Split } from '@/features/usage/search';
import { DAY_MS, HOUR_MS } from '@/lib/time';
import { COST_LOOKS, costParts, type CostPart, type Look } from './context/fill';

/** One stack of the bars: a member of the dimension, its value in each bucket and over the span. */
export interface Stack {
  /** Token kinds retain the shared colour and pattern of a turn's composition. */
  look?: Look;
  id: string;
  label: string;
  /** A second name for it where the first leaves something to say. */
  detail: string | null;
  /** The stack that holds every member beyond the largest ones. */
  rest: boolean;
  values: number[];
  total: number;
  /** For the credits measure: the buckets in which the stack is use that had no rate at all, which read "–" and not 0. */
  unrated: boolean[];
  /** The same over the whole span. */
  allUnrated: boolean;
  /** The upper percentile for a median line; missing observations stay gaps. */
  highs?: number[];
  errorCode?: string;
  counts?: number[];
  count?: number;
}

/** The bars of a span. */
export interface Bars {
  /** The start of each bucket, in milliseconds. */
  xs: number[];
  /** The stacks, the largest first. */
  stacks: Stack[];
  /** The height of each bar: the sum of its stacks. */
  heights: number[];
  bucket: UsageBucket;
  /** How long a bucket lasts, in milliseconds (a month is taken as the days it has from its start). */
  widths: number[];
  lines?: boolean;
  performance?: boolean;
  counts?: number[];
}

/** How long a bucket lasts when it is not a month. */
const FIXED_WIDTH_MS: Record<Exclude<UsageBucket, 'month'>, number> = { hour: HOUR_MS, day: DAY_MS, week: 7 * DAY_MS };

/** Where a month, begun at this moment, ends. */
function monthEnd(start: number): number {
  const date = new Date(start);
  return Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 1);
}

/** How long the bucket that begins at `start` lasts. */
export function bucketWidth(bucket: UsageBucket, start: number): number {
  return bucket === 'month' ? monthEnd(start) - start : FIXED_WIDTH_MS[bucket];
}

/** The last bucket of a span that comes before any use, so that a span of all time does not begin with years of nothing. */
function firstUsed(heights: readonly number[]): number {
  const at = heights.findIndex((height) => height > 0);
  return at < 0 ? 0 : at;
}

/**
 * The bars of an answer for a measure, split by a dimension. Without a split there is one stack, of the
 * totals. With `trim`, the buckets before the first use are left out.
 */
export function barsOf(analytics: UsageAnalytics, measure: Measure, split: Split, t: TFunction, trim = false): Bars {
  if (measure === 'speed' || measure === 'first_token' || measure === 'errors') return performanceBars(analytics, measure, split, t, trim);
  if (measure === 'tokens' && split === 'token_kind') return tokenKindBars(analytics,t,trim);
  const xs = analytics.series.map((point) => Date.parse(point.at));
  const stacks = new Map<string, Stack>();
  const make = (id: string, label: string, detail: string | null, rest: boolean): Stack => {
    const found = stacks.get(id);
    if (found !== undefined) return found;
    const made: Stack = { id, label, detail, rest, values: Array.from({ length: xs.length }, () => 0), total: 0, unrated: Array.from({ length: xs.length }, () => false), allUnrated: false };
    stacks.set(id, made);
    return made;
  };
  const dimension: UsageDimension | null = split === 'none' || split === 'token_kind' ? null : split;
  // What a stack made over the whole span: its credits and the tokens it used without a rate, to tell whether it is all use without one.
  const overSpan = new Map<string, { credits: number; unrated: number; used: boolean }>();
  const take = (stack: Stack, at: number, figures: Parameters<typeof measureOf>[0]) => {
    const value = measureOf(figures, measure);
    stack.values[at] = (stack.values[at] ?? 0) + value;
    stack.total += value;
    const state = creditsState(figures);
    stack.unrated[at] = state.none;
    const sofar = overSpan.get(stack.id) ?? { credits: 0, unrated: 0, used: false };
    overSpan.set(stack.id, { credits: sofar.credits + state.sum, unrated: sofar.unrated + state.unrated, used: sofar.used || figures.turns > 0 || figures.tokens.total_tokens > 0 });
  };

  analytics.series.forEach((point, at) => {
    if (dimension === null || point.by.length === 0) {
      take(make('all', t(`usage.measure.${measure}`), null, false), at, point.figures);
      return;
    }
    for (const part of point.by) {
      take(make(keyId(dimension, part.key), keyLabel(dimension, part.key, t), keyDetail(dimension, part.key, t), isRest(part.key)), at, part.figures);
    }
  });
  for (const stack of stacks.values()) {
    const whole = overSpan.get(stack.id);
    stack.allUnrated = whole !== undefined && whole.used && whole.credits === 0 && whole.unrated > 0;
  }

  // The largest first, the rest of the members last, whatever they add up to.
  const ordered = [...stacks.values()].sort((one, other) => (one.rest === other.rest ? other.total - one.total : one.rest ? 1 : -1));
  const heights = xs.map((_, at) => ordered.reduce((sum, stack) => sum + (stack.values[at] ?? 0), 0));
  const from = trim ? firstUsed(heights) : 0;
  const bucket = analytics.bucket;
  return {
    xs: xs.slice(from),
    stacks: ordered.map((stack) => ({ ...stack, values: stack.values.slice(from), unrated: stack.unrated.slice(from) })),
    heights: heights.slice(from),
    bucket,
    widths: xs.slice(from).map((start) => bucketWidth(bucket, start)),
  };
}

/** The disjoint input/output parts, in the same order and looks as a turn. */
export const TOKEN_KINDS = ['fresh','cache_read','cache_write','output','reasoning'] as const satisfies readonly CostPart[];

function tokenKindBars(analytics:UsageAnalytics,t:TFunction,trim:boolean):Bars {
  const xs=analytics.series.map((point)=>Date.parse(point.at));
  const parts=analytics.series.map((point)=>new Map(costParts(point.figures.tokens,null).map((part)=>[part.part,part.tokens])));
  const stacks=TOKEN_KINDS.map((kind):Stack=>{
    const values=parts.map((parts)=>parts.get(kind) ?? 0);
    return {id:kind,label:t(`contextUsage.cost.name.${kind}`),detail:null,rest:false,values,total:values.reduce((sum,value)=>sum+value,0),unrated:xs.map(()=>false),allUnrated:false,look:COST_LOOKS[kind]};
  });
  const heights=xs.map((_,index)=>stacks.reduce((sum,stack)=>sum+(stack.values[index] ?? 0),0));
  const from=trim ? firstUsed(heights) : 0;
  return {xs:xs.slice(from),stacks:stacks.map((stack)=>({...stack,values:stack.values.slice(from),unrated:stack.unrated.slice(from)})),heights:heights.slice(from),bucket:analytics.bucket,widths:xs.slice(from).map((start)=>bucketWidth(analytics.bucket,start))};
}

/** Medians are read from merged histograms on the server, never added or averaged in a chart. */
function performanceBars(analytics: UsageAnalytics, measure: 'speed' | 'first_token' | 'errors', split: Split, t: TFunction, trim: boolean): Bars {
  const xs = analytics.series.map((point) => Date.parse(point.at));
  const stacks = new Map<string, Stack>();
  const dimension = split === 'none' || split === 'token_kind' ? null : split;
  const errors = (p:UsageAnalytics['totals']['performance'] | undefined) => Object.values(p?.errors ?? {}).reduce((sum,count)=>sum+count,0);
  const read = (p: UsageAnalytics['totals']['performance'] | undefined, high = false):number => {
    if (p === undefined) return Number.NaN;
    if (measure === 'errors') return p.requests>0 ? errors(p)/p.requests : Number.NaN;
    if (measure === 'speed') { const n = high ? p.speed_p90_milli : p.speed_milli; return n === null ? Number.NaN : n / 1000; }
    return (high ? p.first_token_p90_ms : p.first_token_ms) ?? Number.NaN;
  };
  analytics.series.forEach((point, at) => {
    const parts = dimension === null || point.by.length===0 ? [{ key: {}, figures: point.figures }] : point.by;
    for (const part of parts) {
      const id = dimension === null ? 'all' : keyId(dimension, part.key);
      let stack = stacks.get(id);
      if (stack === undefined) {
        const whole = dimension === null ? analytics.totals : analytics.breakdowns.find((table)=>table.by===dimension)?.rows.find((row)=>keyId(dimension,row.key)===id)?.figures;
        stack = { id, label: dimension === null ? t(`usage.measure.${measure}`) : keyLabel(dimension,part.key,t), detail:null, rest:isRest(part.key), total:read(whole?.performance), values:xs.map(()=>Number.NaN), unrated:xs.map(()=>false), allUnrated:false,
          ...(measure === 'errors' ? {counts:xs.map(()=>0),count:errors(whole?.performance)} : {highs:xs.map(()=>Number.NaN)}) };
        stacks.set(id,stack);
      }
      stack.values[at] = read(part.figures.performance);
      if (stack.highs !== undefined) stack.highs[at] = read(part.figures.performance,true);
      if (stack.counts !== undefined) stack.counts[at] = errors(part.figures.performance);
    }
  });
  const heights = analytics.series.map((point)=>read(point.figures.performance));
  const from = trim ? Math.max(0,heights.findIndex(Number.isFinite)) : 0;
  return { xs:xs.slice(from),stacks:[...stacks.values()].sort((a,b)=>Number(a.rest)-Number(b.rest)).map((stack)=>({...stack,values:stack.values.slice(from),highs:stack.highs?.slice(from),counts:stack.counts?.slice(from),unrated:stack.unrated.slice(from)})),heights:heights.slice(from),bucket:analytics.bucket,widths:xs.slice(from).map((start)=>bucketWidth(analytics.bucket,start)),lines:true,performance:true,...(measure==='errors' ? {counts:analytics.series.slice(from).map((point)=>errors(point.figures.performance))} : {}) };
}

// ---------------------------------------------------------------- colours

/** How many colours the chart has before a stack takes the quiet one: the design's eight. */
export const SLOTS = 8;

/** The slot of the colour that marks the stack of every member beyond the largest. */
export const REST_SLOT = 0;

/**
 * Gives each member a colour that it keeps for as long as the chart is split the same way: the first
 * free colour to a member that has none, and a colour a member let go of (it dropped out of the largest)
 * to the next that comes. `given` is what was given before; what comes back is what is given now.
 */
export function assignSlots(given: ReadonlyMap<string, number>, stacks: readonly Pick<Stack, 'id' | 'rest'>[]): Map<string, number> {
  const present = new Set(stacks.map((stack) => stack.id));
  const assigned = new Map([...given].filter(([id]) => present.has(id)));
  const taken = new Set(assigned.values());
  for (const stack of stacks) {
    if (stack.rest) {
      assigned.set(stack.id, REST_SLOT);
      continue;
    }
    if (assigned.has(stack.id)) continue;
    let slot = 1;
    while (taken.has(slot) && slot <= SLOTS) slot += 1;
    // More members than colours cannot be: the answer holds eight. A ninth would repeat the first.
    const given = slot > SLOTS ? ((assigned.size % SLOTS) + 1) : slot;
    assigned.set(stack.id, given);
    taken.add(given);
  }
  return assigned;
}
