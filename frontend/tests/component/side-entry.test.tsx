/** A message's aside entry uses the parent's verified reading mode before it can open the question. */
import { defaultThreadRead } from '@/realtime/threadScope';
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { sideRefusal } from '@/features/side-threads/availability';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { SideReading, Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { threadWriter } from '@/realtime/stores/thread';
import { agentMessage, turn } from '../doubles/thread-rows';
import { show } from './support';

const ask = vi.hoisted(() => vi.fn());
vi.mock('@/features/side-threads/ask', () => ({ openSideAsk: ask }));
const THREAD = 'aside-entry';
const context: TimelineContext = { threadId: THREAD, threadName: 'Review the redirect', detail: 'steps', recent: new Set(['source-turn']), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: false, live: true, lit: null, selection: NOT_PICKING };
const item = { ...agentMessage('source-answer', 'The same-origin test is important.', { turn: 'source-turn' }), thread_id: THREAD, seq_created: 1, seq_updated: 1 };
const thread = { id: THREAD, name: context.threadName, seq: 1, archived: false, lock: null, side_of: null, binding: null,
  access: { permissions: ['thread.view', 'thread.interact'] },
  sides: { open: 0, allowed: true, max: 4, mode: null, reason: 'This harness has no verified way to read without acting.' } } as Thread;
let locale = currentLocale();
let appearance = useAppearance.getState();
beforeEach(() => { locale = currentLocale(); appearance = useAppearance.getState(); ask.mockClear(); threadWriter.evict(THREAD); });
afterEach(async () => { cleanup(); threadWriter.evict(THREAD); useAppearance.setState(appearance); await setLocale(locale); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('an aside entry in %s / %s', (language, theme) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, theme, resolvedTheme: theme }); });
  it('keeps policy permission from standing in for a safe reading mode, then follows newly verified native facts', async () => {
    threadWriter.reset(THREAD, { thread, turns: [turn('source-turn', { ordinal: 1, inputId: 'source-input', state: 'completed' })], items: [item] }, false);
    show(<TimelineContextValue.Provider value={context}><ItemFrame item={item} label={i18n.t('timeline.agent.menu')}><p>The same-origin test is important.</p></ItemFrame></TimelineContextValue.Provider>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.actions.more') }));
    expect(screen.queryByRole('menuitem', { name: i18n.t('side.item.ask') })).not.toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: i18n.t('change.item.fork') })).toBeVisible();
    expect(screen.queryByText(thread.sides?.reason ?? '', { exact: true })).not.toBeInTheDocument();
    expect(ask).not.toHaveBeenCalled();
    act(() => threadWriter.apply(THREAD, { thread: { ...thread, seq: 2, sides: { ...thread.sides!, mode: 'read_only', reason: null } } }));
    const enabled = screen.getByRole('menuitem', { name: i18n.t('side.item.ask') });
    expect(enabled).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(enabled);
    expect(ask).toHaveBeenCalledExactlyOnceWith({ threadId: THREAD, threadName: context.threadName, toTurn: 1, text: '', read: defaultThreadRead(THREAD) });
  });

  it('omits an unsupported read-only question while keeping Fork and a factual reason for a retained draft', async () => {
    const reason='This agent cannot answer without being able to change things.';
    threadWriter.reset(THREAD, { thread: { ...thread, sides: { ...thread.sides!, reason } }, turns: [turn('source-turn', { ordinal: 1, inputId: 'source-input', state: 'completed' })], items: [item] }, false);
    show(<TimelineContextValue.Provider value={context}><ItemFrame item={item} label={i18n.t('timeline.agent.menu')}><p>The same-origin test is important.</p></ItemFrame></TimelineContextValue.Provider>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.actions.more') }));
    expect(screen.queryByRole('menuitem', { name: i18n.t('side.item.ask') })).not.toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: i18n.t('change.item.fork') })).toBeVisible();
    expect(sideRefusal({ ...thread, sides: { ...thread.sides!, reason } }, i18n.t)).toBe(i18n.t('side.unavailable.readOnly'));
    expect(ask).not.toHaveBeenCalled();
  });

  it('translates both current and earlier availability reasons without changing the refusal', () => {
    for (const reason of ["the agent's harness has not said yet what it can do, so it is not known how an aside would read", 'This agent has not reported whether it can answer without changing things yet.']) {
      expect(sideRefusal({ ...thread, sides: { ...thread.sides!, reason } }, i18n.t)).toBe(i18n.t('side.unavailable.notReported'));
    }
    for (const reason of ["the thread's agent profile is gone", 'This thread’s agent profile is no longer available.']) {
      expect(sideRefusal({ ...thread, sides: { ...thread.sides!, reason } }, i18n.t)).toBe(i18n.t('side.unavailable.agentGone'));
    }
  });
});

it.each(['read_only', 'approval_gated', 'no_tools'] satisfies SideReading[])('keeps an already verified %s entry available', async (mode) => {
  threadWriter.reset(THREAD, { thread: { ...thread, sides: { ...thread.sides!, mode, reason: null } }, turns: [turn('source-turn', { ordinal: 1, inputId: 'source-input', state: 'completed' })], items: [item] }, false);
  show(<TimelineContextValue.Provider value={context}><ItemFrame item={item} label={i18n.t('timeline.agent.menu')}><p>The same-origin test is important.</p></ItemFrame></TimelineContextValue.Provider>);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.actions.more') }));
  const choice = screen.getByRole('menuitem', { name: i18n.t('side.item.ask') });
  expect(choice).not.toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(choice);
  expect(ask).toHaveBeenCalledExactlyOnceWith({ threadId: THREAD, threadName: context.threadName, toTurn: 1, text: '', read: defaultThreadRead(THREAD) });
});
