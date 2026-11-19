/** The context ring, the panel it opens and the bar of a turn: tones at their thresholds, the tick, the marks of what was counted and what estimated, and the absent states. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import type * as PerformancePlaces from '@/features/usage/PerformancePlaces';
import type { ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ContextRing } from '@/features/usage/context/ContextRing';
import { ContextPanel } from '@/features/usage/context/ContextPanel';
import { ThreadComposition } from '@/features/usage/context/ThreadComposition';
import { TurnCost } from '@/features/usage/context/TurnCost';
import type { ContextFill, Thread, ThreadContext, ThreadTurnUsage, TokenUsage, Turn, TurnCost as Cost } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';
import { maybeHarnessName } from '../doubles/harness-names';

const answers = vi.hoisted(() => ({ context: null as ThreadContext | null, cost: null as Cost | null, turns: null as ThreadTurnUsage | null }));
vi.mock('@tanstack/react-router', async (load) => ({
  ...await load<typeof Router>(),
  Link: ({ to, children }: { to: string; children: ReactNode }) => <a href={to}>{children}</a>,
}));
vi.mock('@/api/context', () => ({
  threadContextQuery: (thread: string, detail: string, revision: number) => ({ queryKey: ['test-context', thread, detail, revision], queryFn: () => Promise.resolve(answers.context) }),
  turnCostQuery: (thread: string, turn: string, revision: number) => ({ queryKey: ['test-cost', thread, turn, revision], queryFn: () => Promise.resolve(answers.cost) }),
  threadTurnUsageQuery: (thread: string, revision: number) => ({ queryKey: ['test-turns', thread, revision], queryFn: () => Promise.resolve(answers.turns), initialPageParam: null, getNextPageParam: () => undefined }),
}));
vi.mock('@/features/usage/PerformancePlaces', async (original) => ({ ...await original<typeof PerformancePlaces>(), useProviderHealth: vi.fn() }));

const AT = '2026-10-14T10:00:00.000Z';

function threadWith(used: number | null, window: number | null, compactAt: number | null, harness = 'codex'): Thread {
  const usage = used === null ? null : { input_tokens: used, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: used, context_used: used, context_window: window, compact_at: compactAt, updated_at: AT };
  const name = harness === 'claude-code' ? 'Claude Code' : harness === 'codex' ? 'Codex' : 'Research assistant';
  return { id: 'thread-1', name: 'Fix login', usage, binding: {
    agent: { harness_kind: harness, harness: { kind: harness, name } },
    provider: { id: 'context-provider', name: 'Context provider', kind: 'api', harness_kind: null, address: 'acme.context', placement: 'managed', status: 'ready' },
  } } as unknown as Thread;
}

const orchFill: ContextFill = {
  count: 'orch',
  used: 142_000,
  window: 200_000,
  compact_at: 180_000,
  scaled: false,
  at: AT,
  parts: [
    { kind: 'harness', tokens: 20_000, count: 'orch', rows: [] },
    { kind: 'orch_instructions', tokens: 2_000, count: 'orch', rows: [] },
    { kind: 'tool_calls', tokens: 120_000, count: 'orch', rows: [{ kind: 'tool', name: 'exec_command', tokens: 120_000, calls: 4_000, count: 'orch' }] },
  ],
};

beforeEach(() => {
  answers.context = { harness_kind: 'codex', harness: maybeHarnessName('codex'), reports: { tokens: true, context: true, breakdown: false }, fill: orchFill, turn_id: 'turn-1', shortened: { at: AT, before: 170_000, after: 40_000, automatic: null }, can_count: false };
  answers.cost = { turn_id: 'turn-1', rows: [{ model: 'gpt-6-sol', model_display:'Recorded pricing model', tokens: { input_tokens: 10, cached_input_tokens: 0, output_tokens: 2, reasoning_tokens: 0, total_tokens: 12 }, credits: '0.0300', rate_source: 'catalogue' }], credits: '0.0300', estimated_credits: '0.0300', unrated_tokens: 0 };
});

describe('the context ring', () => {
  it('says the share of the window, the tokens and where the conversation is shortened, and marks that point', () => {
    show(<ContextRing thread={threadWith(142_000, 200_000, 180_000)} />);
    const ring = screen.getByRole('button', { name: /Show what fills the context \(Context 142k of 200k \(71%\); shortens at 90%\)/ });
    expect(ring.querySelector('[data-tone="amber"]')).not.toBeNull();
    expect(ring.querySelector('[data-ring-tick]')).not.toBeNull();
  });

  it('is green below 60 % and red above 85 %, and has no tick where the point is not known', () => {
    const { unmount } = show(<ContextRing thread={threadWith(50_000, 200_000, null)} />);
    expect(document.querySelector('[data-context-ring="green"]')).not.toBeNull();
    expect(document.querySelector('[data-ring-tick]')).toBeNull();
    unmount();
    show(<ContextRing thread={threadWith(190_000, 200_000, 180_000)} />);
    expect(document.querySelector('[data-context-ring="red"]')).not.toBeNull();
  });

  it('is absent where the harness reported no size of its prompt', () => {
    show(<ContextRing thread={threadWith(null, null, null)} />);
    expect(screen.queryByRole('button')).toBeNull();
  });
});

describe('the panel it opens', () => {
  it.each(['en', 'zh-CN'] as const)('uses the response’s localized program name in %s', async (locale) => {
    const previous = currentLocale();
    const previousAppearance = useAppearance.getState().locale;
    await setLocale(locale);
    useAppearance.getState().set({ locale });
    try {
      const name = locale === 'en' ? 'Research assistant' : '研究助理';
      const named = { harness: { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '研究助理' } } };
      answers.context = { ...answers.context!, ...named, fill: { ...orchFill, count: 'counted' } };
      show(<ContextPanel thread={threadWith(142_000, 200_000, 180_000)} />);
      expect(await screen.findByText(i18n.t('contextUsage.count.counted', { harness: name }))).toBeVisible();
      expect(screen.queryByText('research-agent')).not.toBeInTheDocument();
    } finally {
      useAppearance.getState().set({ locale: previousAppearance });
      await setLocale(previous);
    }
  });

  it('shows Orch’s estimate by part, marked so, with the rest named the harness’s own, then free space and the reserve', async () => {
    const user = userEvent.setup();
    show(<ContextRing thread={threadWith(142_000, 200_000, 180_000)} />);
    await user.click(screen.getByRole('button', { name: /Show what fills the context/ }));
    const panel = await screen.findByRole('dialog');
    await within(panel).findByText('Codex\'s own instructions and tools');
    expect(within(panel).getByText('estimated by Orch')).toBeInTheDocument();
    expect(within(panel).getByText('Tool calls and their results')).toBeInTheDocument();
    expect(within(panel).getByText('Free')).toBeInTheDocument();
    expect(within(panel).getByText('Kept free for shortening')).toBeInTheDocument();
    // Every part is reached by the keyboard and says its numbers.
    expect(within(panel).getByRole('img', { name: /Tool calls and their results: 120k, 60%/ })).toHaveAttribute('tabindex', '0');
    expect(within(panel).getByText(/freed 130k/)).toBeInTheDocument();
    await user.click(within(panel).getByRole('button', { name: /Show its 1 part/ }));
    expect(within(panel).getByText('exec_command')).toBeInTheDocument();
  });

  it('marks a harness’s own count as counted by it, and says so where the harness reports no size at all', async () => {
    const user = userEvent.setup();
    answers.context = { ...answers.context!, harness_kind: 'claude-code', harness: maybeHarnessName('claude-code'), reports: { tokens: true, context: true, breakdown: true }, fill: { ...orchFill, count: 'counted', parts: orchFill.parts.map((part) => ({ ...part, count: 'counted' as const })) } };
    const { unmount } = show(<ContextRing thread={threadWith(142_000, 200_000, 180_000, 'claude-code')} />);
    await user.click(screen.getByRole('button', { name: /Show what fills the context/ }));
    expect(await screen.findByText('counted by Claude Code')).toBeInTheDocument();
    unmount();
    answers.context = { ...answers.context, harness_kind: 'codex', harness: maybeHarnessName('codex'), reports: { tokens: true, context: false, breakdown: false }, fill: null, shortened: null };
    show(<ContextRing thread={threadWith(1_000, 200_000, null)} />);
    await user.click(screen.getByRole('button', { name: /Show what fills the context/ }));
    expect(await screen.findByText('Codex does not report how full its context is.')).toBeInTheDocument();
  });
});

describe('the bar of a turn', () => {
  const usage: TokenUsage = { input_tokens: 10_000, cached_input_tokens: 6_000, cache_write_tokens: 1_000, output_tokens: 2_000, reasoning_tokens: 500, total_tokens: 12_000 };
  const turn = (over: Partial<Turn> = {}): Turn => ({
    id: 'turn-1', thread_id: 'thread-1', segment_id: 'segment-1', ordinal: 3, state: 'completed', input_id: 'input-1', before_snapshot_id: null,
    model: 'gpt-6-sol', model_display: 'GPT-6 Sol', effort: null, params: {}, error: null, usage, command_cleanup: null,
    usage_detail: { subagents: { input_tokens: 2_000, cached_input_tokens: 1_000, output_tokens: 1_000, reasoning_tokens: 0, total_tokens: 3_000 }, by_subagent: [{ stream: 'sub:helper', tokens: { input_tokens: 2_000, cached_input_tokens: 1_000, output_tokens: 1_000, reasoning_tokens: 0, total_tokens: 3_000 } }], models: [], context_window: 200_000, context_used: 142_000, compact_at: 180_000, shortenings: [], breakdown: false },
    context: null, interrupted_by: null, limit_bypass: [], phase: null, plan: false, started_at: AT, completed_at: AT, waiting_since: null, waited_ms: 0, seq_updated: 4,
    ...over,
  });

  it.each([
    ['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark'],
  ] as const)('omits empty breakdown brackets for any total and retains real details (%s, %s)', async (locale, theme) => {
    const previousLocale = currentLocale();
    const previousTheme = document.documentElement.getAttribute('data-theme');
    await setLocale(locale);
    document.documentElement.setAttribute('data-theme', theme);
    const withoutBreakdown: TokenUsage = { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0 };
    const view = show(<TurnCost threadId="thread-1" turn={turn({ usage: withoutBreakdown, usage_detail: null })} />);
    try {
      // A reported total can arrive without its parts; summary presence decides the label, not zero.
      for (const total of [0, 42]) {
        view.rerender(<TurnCost threadId="thread-1" turn={turn({ usage: { ...withoutBreakdown, total_tokens: total }, usage_detail: null })} />);
        const tokens = locale === 'en' ? `${total} tokens` : `${total} 个令牌`;
        const label = locale === 'en' ? `Show what the turn used: ${tokens}` : `查看本轮的用量：${tokens}`;
        expect(screen.getByRole('button', { name: label })).toHaveTextContent(tokens);
      }
      view.rerender(<TurnCost threadId="thread-1" turn={turn({ usage: { ...withoutBreakdown, input_tokens: 7, total_tokens: 7 }, usage_detail: null })} />);
      const detailed = locale === 'en' ? 'Show what the turn used: 7 tokens (fresh input 7)' : '查看本轮的用量：7 个令牌（新输入 7）';
      expect(screen.getByRole('button', { name: detailed })).toBeInTheDocument();
    } finally {
      view.unmount();
      await setLocale(previousLocale);
      if (previousTheme === null) document.documentElement.removeAttribute('data-theme');
      else document.documentElement.setAttribute('data-theme', previousTheme);
    }
  });

  it('draws the parts of the turn with its sub-agents apart, says them under the pointer, and opens its details', async () => {
    const user = userEvent.setup();
    show(<TurnCost threadId="thread-1" turn={turn()} />);
    const open = screen.getByRole('button', { name: /Show what the turn used: 12k tokens/ });
    expect(open).toHaveClass('min-h-6', 'min-w-6');
    expect(open.querySelectorAll('[data-part]')).toHaveLength(6);
    expect(open.getAttribute('aria-label')).toMatch(/sub-agents 3,000/);
    expect(open.getAttribute('aria-label')).toMatch(/60% of the input from the cache/);
    await user.click(open);
    const details = await screen.findByRole('dialog');
    expect(within(details).getByText('By model')).toBeInTheDocument();
    expect(within(details).getByText('By sub-agent')).toBeInTheDocument();
    expect(await within(details).findByText(/≈ 0.03 credits/)).toBeInTheDocument();
    expect(within(details).getByText('Recorded pricing model')).toBeVisible();
    expect(details).not.toHaveTextContent('gpt-6-sol');
    expect(within(details).getByText(/142k of 200k \(71%\)/)).toBeInTheDocument();
  });

  it('keeps measured main model time and explains delegated and unmeasured tool intervals', async () => {
    if (answers.cost === null) throw new Error('a cost answer is provided');
    answers.cost.performance = { model_ms: 3000, tool_ms: 7000, first_output_ms: 1200, speed_milli: 64_000, requests: 3, reasoning_hidden: true, tool_includes_subagents: true, tool_includes_unmeasured: true };
    const user = userEvent.setup();
    show(<TurnCost threadId="thread-1" turn={turn()} />);
    await user.click(screen.getByRole('button', { name: /Show what the turn used: 12k tokens/ }));
    const details = await screen.findByRole('dialog');
    expect(await within(details).findByText('3.0 s')).toBeInTheDocument();
    expect(within(details).getByText('7.0 s')).toBeInTheDocument();
    expect(within(details).getByText('Includes thinking')).toBeInTheDocument();
    expect(within(details).getByText('Includes sub-agent time.')).toBeInTheDocument();
    expect(within(details).getByText('Includes intervals without measured model time.')).toBeInTheDocument();
  });

  it('does not turn an unnamed cost row into its model identifier or a parent’s cached label',async()=>{
    if(answers.cost===null)throw new Error('A cost reply is provided');
    answers.cost.rows=answers.cost.rows.map((row)=>({...row,model:'private-price-route',model_display:null}));
    show(<TurnCost threadId="thread-1" turn={turn()} />);
    await userEvent.click(screen.getByRole('button',{name:/Show what the turn used/}));
    const details=await screen.findByRole('dialog');
    expect(await within(details).findByText(i18n.t('composer.model.displayUnknown'))).toBeVisible();
    expect(details).not.toHaveTextContent('private-price-route');
  });

  it('is absent from a turn whose harness reported nothing', () => {
    show(<TurnCost threadId="thread-1" turn={turn({ usage: null, usage_detail: null })} />);
    expect(screen.queryByRole('button')).toBeNull();
  });
});

describe('the turns over time', () => {
  const row = (ordinal: number, fill: number | null, shortened: boolean) => ({
    turn_id: `turn-${ordinal}`, ordinal, started_at: AT, completed_at: AT, model: 'gpt-6-sol', model_display: 'GPT-6 Sol',
    usage: { input_tokens: 10_000, cached_input_tokens: 4_000, output_tokens: 1_000, reasoning_tokens: 200, total_tokens: 11_000 },
    detail: { subagents: null, by_subagent: [], models: [], context_window: fill === null ? null : 200_000, context_used: fill, compact_at: null, shortenings: shortened ? [{ at: AT, before: 180_000, after: 30_000, automatic: true }] : [], breakdown: false },
  });

  it('draws each turn as a column the keyboard reaches, marks the shortenings, and says the same in a table', async () => {
    answers.turns = { harness_kind: 'codex', harness: maybeHarnessName('codex'), reports: { tokens: true, context: true, breakdown: false }, turns: [row(1, 100_000, false), row(2, 30_000, true)], next_before: null };
    show(<ThreadComposition thread={threadWith(30_000, 200_000, null)} />);
    const second = await screen.findByRole('img', { name: /^Turn 2: 11k, .*context 15% full, and the conversation was shortened$/ });
    expect(second).toHaveAttribute('tabindex', '0');
    expect(document.querySelectorAll('[data-usage-shortened]')).toHaveLength(1);
    expect(document.querySelector('[data-usage-fill-line]')).not.toBeNull();
    expect(screen.getByRole('table', { name: 'Each turn\'s tokens, cache hit rate and context' })).toBeInTheDocument();
    expect(screen.getByText(/40% of the input read from the cache/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Export as CSV' })).toBeInTheDocument();
  });

  it('says by the harness’s name what it does not report, and draws nothing', async () => {
    answers.turns = { harness_kind: 'codex', harness: maybeHarnessName('codex'), reports: { tokens: false, context: false, breakdown: false }, turns: [], next_before: null };
    show(<ThreadComposition thread={threadWith(null, null, null)} />);
    expect(await screen.findByText('Codex reports no use of tokens on this thread, so none is shown.')).toBeInTheDocument();
    expect(document.querySelector('[data-usage-chart]')).toBeNull();
  });
});
