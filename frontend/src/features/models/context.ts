/** A context in tokens as people write it ("1M", "400K"), the round sizes a thread may be given between a model's own window and its largest, and where a size crosses a step of the model's rate.
 *
 * Both the provider's Models tab and the thread's model control speak of a context, so the way it is
 * written, the sizes that are offered and the steps of a rate that a size lies above are worked out once,
 * here. Nothing in this file states a window of any model: every figure comes from the model's entry.
 */
import type { Rate } from '@/generated/api-types';
import { formatNumber } from '@/lib/format';

const THOUSAND = 1_000;
const MILLION = 1_000_000;

/** How many decimals a figure in thousands or millions shows at most: "1.05M", "12.5K". */
const DECIMALS_AT_MOST = 2;

/** A size of context as people say it: "1M", "400K", "828K", "12.5K", and in full below a thousand. */
export function formatContext(tokens: number, locale: string): string {
  if (tokens >= MILLION) return `${formatNumber(tokens / MILLION, locale, { maximumFractionDigits: DECIMALS_AT_MOST })}M`;
  if (tokens >= THOUSAND) return `${formatNumber(tokens / THOUSAND, locale, { maximumFractionDigits: 1 })}K`;
  return formatNumber(tokens, locale);
}

/**
 * Sizes that a context is usually asked for. They only decide which sizes lie between a model's own window
 * and its largest context to be offered as choices; nobody is held to them, and the field of the control
 * takes any other number.
 */
const ROUND_SIZES: readonly number[] = [32_000, 64_000, 128_000, 200_000, 256_000, 400_000, 512_000, 800_000];

/** How many round sizes are offered between the two ends at most: a list of choices stays short enough to read at a glance. */
const BETWEEN_MOST = 3;

export interface ContextEnds {
  /** The least a context may be, as the adapter's parameter declares it. */
  least: number;
  /** The model's own window, where its entry gives one. */
  own: number | null;
  /** The largest the model's managers opened. */
  largest: number;
}

/** The sizes offered for a context, the largest first: the largest, the model's own window, and a few round sizes between them. */
export function contextChoices({ least, own, largest }: ContextEnds): number[] {
  const floor = own !== null && own >= least ? own : least;
  const between = ROUND_SIZES.filter((size) => size > floor && size < largest);
  const kept =
    between.length <= BETWEEN_MOST
      ? between
      : Array.from({ length: BETWEEN_MOST }, (_, index) => between[Math.round((index * (between.length - 1)) / (BETWEEN_MOST - 1))] ?? 0);
  const sizes = new Set<number>([largest, ...kept]);
  if (own !== null && own >= least && own < largest) sizes.add(own);
  return [...sizes].sort((one, other) => other - one);
}

/** The step of a rate that a size lies above: the largest `context_tokens` of its steps that is below the size; none when the size pays the usual prices. */
export function stepBelow(rate: Pick<Rate, 'above'> | null | undefined, size: number): number | null {
  const steps = (rate?.above ?? []).map((step) => step.context_tokens).filter((tokens) => tokens < size);
  return steps.length === 0 ? null : Math.max(...steps);
}

/** The first step of a rate, the one a prompt reaches soonest; none when the rate has no step. */
export function firstStep(rate: Pick<Rate, 'above'> | null | undefined): number | null {
  const steps = (rate?.above ?? []).map((step) => step.context_tokens);
  return steps.length === 0 ? null : Math.min(...steps);
}

/** A whole number of tokens written as the person typed it, or null when it is none. Spaces and the grouping marks people put in a long number are let through. */
export function readTokens(text: string): number | null {
  const bare = text.trim().replace(/[\s,_]/g, '');
  if (!/^\d+$/.test(bare)) return null;
  const tokens = Number(bare);
  return Number.isSafeInteger(tokens) ? tokens : null;
}
