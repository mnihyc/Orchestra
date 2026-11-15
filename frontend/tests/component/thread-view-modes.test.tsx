/** The ways of reading share one portable thread preference and the native tool renderer. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import { client } from '@/api/client';
import { DetailControl } from '@/features/thread/DetailControl';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { changedTimeline, useTimelinePreferences } from '@/features/timeline/preferences';
import { timelineUi } from '@/features/timeline/state';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { command } from '../doubles/thread-rows';
import { show } from './support';

const wire = vi.hoisted(() => { const state: Record<string, unknown> = {}; return { state, save: vi.fn<typeof Personal.setMeState>() }; });
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], staleTime: Infinity, queryFn: () => Promise.resolve(wire.state) },
  setMeState: (...args: Parameters<typeof Personal.setMeState>) => wire.save(...args),
}));
const session = client.session.getState();
const caches: QueryClient[] = [];
const threadIds: string[] = [];
beforeEach(() => {
  wire.state = { timeline: { diff_layout: 'two', extra: 'kept', threads: { b: { work_detail: 'verbose', extra: 'also kept' } } } };
  wire.save.mockReset().mockImplementation((key, value) => { wire.state = { ...wire.state, [key]: value }; return Promise.resolve(); });
  client.session.setState({ status: 'signed_in', userId: 'detail-reader', readerVersion: 1 });
});
afterEach(async () => { cleanup(); for (const cache of caches.splice(0)) cache.clear(); for (const id of threadIds.splice(0)) { threadWriter.evict(id); timelineUi.forget(id); } client.session.setState(session); await setLocale('en'); });
function Choice({ id }: { id: string }) { return <section aria-label={`Thread ${id}`}><DetailControl preferences={useTimelinePreferences(id)} /></section>; }
function draw() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  return show(<QueryClientProvider client={cache}><Choice id="a" /><Choice id="b" /></QueryClientProvider>);
}

it.each(['en', 'zh-CN'] as const)('keeps the way of reading per thread across another device without losing unrelated preferences in %s', async (locale) => {
  await setLocale(locale); const view = draw();
  const a = within(screen.getByRole('region', { name: 'Thread a' }));
  const b = within(screen.getByRole('region', { name: 'Thread b' }));
  await waitFor(() => expect(b.getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.verbose')));
  expect(a.getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.normal'));
  await userEvent.click(a.getByRole('combobox')); await userEvent.click(screen.getByRole('option', { name: i18n.t('thread.viewModes.diagnostics') }));
  await waitFor(() => expect(wire.save).toHaveBeenCalledWith('timeline', expect.objectContaining({ extra: 'kept', diff_layout: 'two', threads: {
    a: { work_detail: 'diagnostics' }, b: { work_detail: 'verbose', extra: 'also kept' },
  } })));
  expect(b.getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.verbose'));
  await userEvent.click(a.getByRole('combobox')); await userEvent.click(screen.getByRole('option', { name: i18n.t('thread.viewModes.verbose') }));
  await waitFor(() => expect(wire.save).toHaveBeenCalledTimes(2));
  view.unmount(); draw();
  await waitFor(() => expect(within(screen.getByRole('region', { name: 'Thread a' })).getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.verbose')));
  expect(changedTimeline(wire.state.timeline, { value: { diff_layout: 'one' } })).toMatchObject({ extra: 'kept', threads: { a: { work_detail: 'verbose' }, b: { extra: 'also kept' } } });
});

it.each(['en', 'zh-CN'] as const)('offers Normal, Verbose and Diagnostics, and reads what an earlier version kept as Normal in %s', async (locale) => {
  wire.state = { timeline: { threads: { a: { work_detail: 'summary', live_tools: true }, b: { work_detail: 'everything', diagnostics: true } } } };
  await setLocale(locale); draw();
  const a = within(screen.getByRole('region', { name: 'Thread a' }));
  await waitFor(() => expect(a.getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.normal')));
  expect(within(screen.getByRole('region', { name: 'Thread b' })).getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.normal'));
  await userEvent.click(a.getByRole('combobox'));
  expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual(
    ['normal', 'verbose', 'diagnostics'].map((mode): unknown => expect.stringContaining(i18n.t(`thread.viewModes.${mode}`))));
  await userEvent.click(screen.getByRole('option', { name: i18n.t('thread.viewModes.diagnostics') }));
  await waitFor(() => expect(wire.save).toHaveBeenCalledWith('timeline', expect.objectContaining({ threads: expect.objectContaining({
    a: { work_detail: 'diagnostics', live_tools: true } }) as unknown })));
  await waitFor(() => expect(a.getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.diagnostics')));
  expect(within(screen.getByRole('region', { name: 'Thread b' })).getByRole('combobox')).toHaveTextContent(i18n.t('thread.viewModes.normal'));
});

for (const [mode, open] of [['verbose', true], ['diagnostics', true], ['normal', false]] as const) {
  it.each(['en', 'zh-CN'] as const)(`${open ? 'opens' : 'keeps closed'} current tool output in ${mode} reading, and keeps a deliberate choice in %s`, async (locale) => {
    await setLocale(locale);
    const threadId = `live-detail-${mode}-${locale}`; threadIds.push(threadId);
    const item = { ...command('live-command', { command: 'check files', output: 'Current streamed output', exit: null }), thread_id: threadId, status: 'in_progress' as const };
    threadWriter.reset(threadId, { items: [item] }, false);
    const context: TimelineContext = { threadId, threadName: 'Live work', detail: mode, layout: 'one', setLayout: () => undefined,
      meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };
    const view = show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={threadId} id={item.id} /></TimelineContextValue.Provider>);
    const control = view.container.querySelector<HTMLButtonElement>('button[aria-expanded]');
    if (control === null) throw new Error('The actual tool row must retain its native disclosure');
    expect(control).toHaveAttribute('aria-expanded', String(open));
    if (open) expect(screen.getByText('Current streamed output')).toBeVisible();
    else expect(screen.queryByText('Current streamed output')).not.toBeInTheDocument();
    await userEvent.click(control); expect(control).toHaveAttribute('aria-expanded', String(!open));
    if (open) expect(screen.queryByText('Current streamed output')).not.toBeInTheDocument();
    else expect(screen.getByText('Current streamed output')).toBeVisible();
  });
}
