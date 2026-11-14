/** The context ring is where the size of the context is chosen, and a thread that is only looked at still shows its figure. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type { ContextState } from '@/features/composer/modelState';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ContextRing } from '@/features/usage/context/ContextRing';
import { show } from './support';

const fake = vi.hoisted(() => ({ choose: vi.fn(), states: [] as unknown[] }));
vi.mock('@/features/composer/runsWith', () => ({ useRunsWith: () => ({ states: fake.states, ready: true, choosing: false, chooser: () => null, choose: fake.choose, chooseError: null }) }));
vi.mock('@/features/usage/context/ContextPanel', () => ({ ContextPanel: () => <p>What fills the context</p> }));

const state = (largest: number): ContextState => ({ kind: 'context', key: 'context', title: 'Context', description: '', apply: 'next_turn', writable: true, lockedBy: null, place: null, from: 'start', stale: false,
  least: 1_000, window: 272_000, largest, start: 272_000, chosen: null, force: 272_000, rate: null });
const usage = { input_tokens: 1_000, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 1_000, context_used: 54_400, context_window: 272_000, compact_at: null, updated_at: '2026-10-14T10:00:00.000Z' };
const thread = (used: boolean): Thread => ({ id: 'thread-1', usage: used ? usage : null, access: { permissions: ['thread.view', 'thread.interact', 'thread.manage'] }, binding: { agent: { name: 'Agent' } } }) as unknown as Thread;

beforeEach(() => { fake.choose.mockClear(); fake.states = [state(1_000_000)]; });
afterEach(async () => { cleanup(); await setLocale('en'); });

it('shows the figure of a thread that is only looked at, and keeps its size from being changed', async () => {
  show(<ContextRing thread={thread(true)} side="top" readOnly />);
  const ring = document.querySelector('[data-context-ring]');
  expect(ring).not.toBeNull(); expect(ring).toHaveTextContent('20%');
  await userEvent.click(screen.getByRole('button', { name: /20/ }));
  const sizes = await screen.findAllByRole('radio');
  expect(sizes.length).toBeGreaterThan(1);
  for (const size of sizes) expect(size).toBeDisabled();
  expect(within(document.body).getByText('What fills the context')).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) it(`holds the choice of the size where the model has more than one, even before the harness has reported anything (${locale})`, async () => {
  await setLocale(locale);
  show(<ContextRing thread={thread(false)} side="top" />);
  const chip = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.context.title'), value: '272K' }) });
  expect(chip).toHaveTextContent('272K');
  await userEvent.click(chip);
  await userEvent.click(await screen.findByRole('radio', { name: '1M' }));
  expect(fake.choose).toHaveBeenCalledWith(fake.states[0], 1_000_000);
  expect(screen.queryByText('What fills the context')).not.toBeInTheDocument();
});

it('shows the fixed model window from the beginning before any usage is reported', async () => {
  fake.states = [state(272_000)];
  show(<ContextRing thread={thread(false)} side="top" />);
  expect(document.querySelector('[data-context-ring]')).toHaveTextContent('272K');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.context.title'), value: '272K' }) }));
  expect(screen.getByText('What fills the context')).toBeVisible();
  expect(screen.queryByRole('radio')).not.toBeInTheDocument();
  expect(fake.choose).not.toHaveBeenCalled();
});

it('keeps the context affordance visible with an honest unreported state before a window is known', async () => {
  fake.states = [];
  show(<ContextRing thread={thread(false)} side="top" />);
  const control = screen.getByRole('button', { name: i18n.t('contextUsage.ring.open', { label: i18n.t('contextUsage.ring.notReported') }) });
  expect(control).toHaveTextContent(i18n.t('composer.model.context.title'));
  await userEvent.click(control);
  expect(screen.getByText('What fills the context')).toBeVisible();
});

it('keeps a fixed context visible and read-only before the first reported use', async () => {
  fake.states = [state(272_000)];
  show(<ContextRing thread={thread(false)} side="top" readOnly />);
  const control = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.context.title'), value: '272K' }) });
  await userEvent.click(control);
  expect(screen.getByText('What fills the context')).toBeVisible();
  expect(screen.queryByRole('radio')).not.toBeInTheDocument();
  expect(fake.choose).not.toHaveBeenCalled();
});
