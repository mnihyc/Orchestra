/** One definition for each live value, its words, permitted places and inspection subject. */
import type { TFunction } from 'i18next';
import type { Environment, Host, Provider, ResourceSample, Thread, Turn } from '@/generated/api-types';
import type { ResourceSubject } from '@/features/resource-facts/facts';
import { providerHint } from '@/features/resource-facts/facts';
import { turnActiveMs, turnWaiting } from '@/features/timeline/turnTime';
import { formatCredits } from '@/features/usage/figures';
import { formatBytes, formatNumber, formatPercent, formatTokens } from '@/lib/format';
import { formatSpan, type TimeContext } from '@/lib/time';
import { estimateTokenCounts, type CharacterCounts } from '@/lib/tokenEstimate';

export const STATUS_PLACES = ['bar', 'header', 'live'] as const;
export type StatusPlace = typeof STATUS_PLACES[number];
export interface StatusValue {
  text: string;
  subject?: ResourceSubject;
  warning?: boolean;
  details?: { name: string; value: string }[];
}
export interface StatusContext extends TimeContext {
  t: TFunction;
  now: number;
  thread: Thread;
  turn: Turn | null;
  outputCounts: CharacterCounts | null;
  /** The current turn's model estimate, when its option has been read. */
  tokenEstimate: unknown;
  provider: Provider | undefined;
  environment: Environment | undefined;
  sample: ResourceSample | null;
  host: Host | undefined;
  activeLabel?: string | undefined;
}
const currentTurn = (context: StatusContext) => context.turn !== null && context.thread.current_turn_id === context.turn.id && context.thread.state !== 'idle' && !context.thread.archived;
export const generating = (context: StatusContext) => currentTurn(context) && context.turn !== null && context.thread.state === 'running'
  && context.turn.state === 'running' && !turnWaiting(context.turn) && context.turn.phase?.kind !== 'compacting';

export function outputSpeed(context: StatusContext): { speed: number; estimated: boolean } | null {
  if (!generating(context) || context.turn === null) return null;
  const { turn } = context;
  const reported = turn.usage === null ? 0 : Math.max(0, turn.usage.output_tokens - turn.usage.reasoning_tokens);
  const output = reported > 0 ? reported : estimatedOutput(context);
  const milliseconds = turn.usage_detail?.api_time_ms ?? turnActiveMs(turn, context.now);
  if (output === null || output <= 0 || milliseconds === null || milliseconds <= 0) return null;
  return { speed: output * 1000 / milliseconds, estimated: reported <= 0 };
}

function estimatedOutput(context: StatusContext): number | null {
  const counts = context.outputCounts;
  return counts === null ? null : estimateTokenCounts(counts.ascii, counts.other, context.tokenEstimate);
}

function cost(context: StatusContext, of: 'turn' | 'thread'): StatusValue | null {
  const current = context.thread.costs?.turn;
  const amounts = of === 'thread' ? context.thread.costs?.total : currentTurn(context) && current?.turn_id === context.turn?.id ? current : undefined;
  if (amounts?.credits === null || amounts?.credits === undefined) return null;
  const value = Number(amounts.credits);
  if (!Number.isFinite(value)) return null;
  const mark = amounts.unrated_tokens > 0 ? '≥' : Number(amounts.estimated_credits) > 0 ? '≈' : '';
  return { text: context.t(of === 'turn' ? 'statusItems.turnCostValue' : 'statusItems.threadCostValue', { value: `${mark}${formatCredits(value, context.locale)}`, unit: context.t('caps.unit.credits') }), details: [
    { name: context.t('statusItems.recorded'), value: formatCredits(value, context.locale) },
    { name: context.t('statusItems.estimated'), value: formatCredits(Number(amounts.estimated_credits), context.locale) },
    { name: context.t('statusItems.unrated'), value: formatTokens(amounts.unrated_tokens, context.locale) },
  ] };
}

function tokens(context: StatusContext): StatusValue | null {
  if (!currentTurn(context) || context.turn === null) return null;
  const usage = context.turn.usage;
  if (usage !== null && usage.output_tokens > 0) return { text: context.t('statusItems.tokens', { value: formatTokens(usage.total_tokens, context.locale) }), details: [
    { name: context.t('statusItems.input'), value: formatTokens(usage.input_tokens, context.locale) },
    { name: context.t('statusItems.output'), value: formatTokens(usage.output_tokens, context.locale) },
  ] };
  const output = estimatedOutput(context);
  if (output === null || output === 0) return usage === null ? null : { text: context.t('statusItems.tokens', { value: formatTokens(usage.total_tokens, context.locale) }) };
  return { text: usage === null ? `≈${context.t('statusItems.outputTokens', { value: formatTokens(output, context.locale) })}` : `≈${context.t('statusItems.tokens', { value: formatTokens(usage.input_tokens + output, context.locale) })}`, details: [
    { name: context.t('statusItems.input'), value: usage === null ? context.t('usagePerf.dash') : formatTokens(usage.input_tokens, context.locale) },
    { name: context.t('statusItems.output'), value: `≈${formatTokens(output, context.locale)}` },
  ] };
}
const everyPlace: readonly StatusPlace[] = STATUS_PLACES;
export const STATUS_ITEMS = [
  { id: 'phase', title: 'statusItems.items.phase', places: everyPlace, place: 'live', shown: true,
    read: (c: StatusContext): StatusValue | null => generating(c) ? { text: c.activeLabel ?? c.t('statusItems.generating') } : null },
  { id: 'speed', title: 'statusItems.items.speed', places: everyPlace, place: 'live', shown: true,
    read: (c: StatusContext): StatusValue | null => { const value = outputSpeed(c); return value === null ? null : { text: `${value.estimated ? '≈' : ''}${c.t('statusItems.speed', { value: formatNumber(value.speed, c.locale, { maximumFractionDigits: 1 }) })}`, details: [{ name: c.t('statusItems.source'), value: c.t(value.estimated ? 'statusItems.streamEstimate' : 'statusItems.reported') }] }; } },
  { id: 'elapsed', title: 'statusItems.items.elapsed', places: everyPlace, place: 'live', shown: true,
    read: (c: StatusContext): StatusValue | null => { const elapsed = c.turn === null ? null : turnActiveMs(c.turn, c.now); return currentTurn(c) && elapsed !== null ? { text: formatSpan(elapsed, c) } : null; } },
  { id: 'tokens', title: 'statusItems.items.tokens', places: everyPlace, place: 'live', shown: true,
    read: tokens },
  { id: 'turn_cost', title: 'statusItems.items.turn_cost', places: everyPlace, place: 'bar', shown: false, read: (c: StatusContext) => cost(c, 'turn') },
  { id: 'thread_cost', title: 'statusItems.items.thread_cost', places: everyPlace, place: 'bar', shown: false, read: (c: StatusContext) => cost(c, 'thread') },
  { id: 'context', title: 'statusItems.items.context', places: everyPlace, place: 'bar', shown: true,
    read: (c: StatusContext): StatusValue | null => { const used = c.turn?.context?.used ?? c.thread.usage?.context_used, window = c.turn?.context?.window ?? c.thread.usage?.context_window; return used === null || used === undefined || window === null || window === undefined || window <= 0 ? null : { text: c.t('statusItems.context', { value: `${c.turn?.context?.count === 'orch' || c.turn?.context?.count === 'estimated' ? '≈' : ''}${formatPercent(used / window, c.locale)}` }), details: [{ name: c.t('resourceFacts.context'), value: `${formatTokens(used, c.locale)} / ${formatTokens(window, c.locale)}` }] }; } },
  { id: 'provider_window', title: 'statusItems.items.provider_window', places: everyPlace, place: 'bar', shown: true,
    read: (c: StatusContext): StatusValue | null => { if (c.provider === undefined) return null; const hint = providerHint(c.provider, c, 'window'); return hint.text === '' ? null : { text: hint.text, warning: hint.warning, subject: { kind: 'provider', provider: c.provider } }; } },
  { id: 'provider_running', title: 'statusItems.items.provider_running', places: everyPlace, place: 'bar', shown: false,
    read: (c: StatusContext): StatusValue | null => !c.provider?.access.permissions.includes('provider.view') ? null : { text: c.t('resourceFacts.running', { count: c.provider.running_threads, value: formatNumber(c.provider.running_threads, c.locale) }), subject: { kind: 'provider', provider: c.provider } } },
  { id: 'environment_cpu', title: 'statusItems.items.environment_cpu', places: everyPlace, place: 'bar', shown: false,
    read: (c: StatusContext): StatusValue | null => c.environment === undefined || c.sample === null ? null : { text: c.t('statusItems.cpu', { value: formatPercent(c.sample.cpu_percent / 100, c.locale) }), subject: { kind: 'environment', environment: c.environment, sample: c.sample } } },
  { id: 'environment_memory', title: 'statusItems.items.environment_memory', places: everyPlace, place: 'bar', shown: false,
    read: (c: StatusContext): StatusValue | null => c.environment === undefined || c.sample === null ? null : { text: c.t('statusItems.memory', { value: formatBytes(c.sample.memory_bytes, c.locale) }), subject: { kind: 'environment', environment: c.environment, sample: c.sample } } },
  { id: 'host_load', title: 'statusItems.items.host_load', places: everyPlace, place: 'bar', shown: false,
    read: (c: StatusContext): StatusValue | null => c.host?.machine.online !== true || c.host.usage?.cpu_percent_milli === null || c.host.usage?.cpu_percent_milli === undefined ? null : { text: c.t('statusItems.hostLoad', { value: formatPercent(c.host.usage.cpu_percent_milli / 100_000, c.locale) }), subject: { kind: 'host', host: c.host } } },
] as const;
export type StatusItemId = typeof STATUS_ITEMS[number]['id'];
