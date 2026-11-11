/** A selected diff line reaches the private draft and one ordinary review input, with reader fences. */
import { createHash, webcrypto } from 'node:crypto';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Source from '@/api/source';
import type * as Threads from '@/api/threads';
import type * as Settings from '@/api/settings';
import { composerWriter, useComposerStore } from '@/features/composer/Draft';
import { client } from '@/api/client';
import { threadKeys } from '@/api/threads';
import { ReviewableDiff, ReviewTray } from '@/features/source/Review';
import { reviewMessage } from '@/features/source/words';
import type { Draft, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { threadDraft, threadInput } from '../doubles/thread-rows';
import { snapshot } from '../unit/continuation-fixtures';
import { deferred } from './deferred';
import { show } from './support';

const wire = vi.hoisted(() => ({ draft: null as Draft | null, settings: [] as ResolvedSetting[], load: vi.fn<() => Promise<Draft | null>>(), save: vi.fn<typeof Source.saveReviewComments>(), send: vi.fn<typeof Threads.sendInput>() }));
vi.mock('@/api/threads', async (original) => {
  const actual = await original<typeof Threads>();
  return { ...actual, draftQuery: (id: string) => ({ ...actual.draftQuery(id), queryFn: wire.load }), sendInput: wire.send };
});
vi.mock('@/api/settings', async (original) => ({ ...await original<typeof Settings>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve(wire.settings), staleTime: Infinity }) }));
vi.mock('@/api/source', async (original) => ({ ...await original<typeof Source>(), saveReviewComments: wire.save }));
const DIFF = '@@ -10,2 +10,2 @@\n-old condition\n+new condition\n keep this\n';
const savedSession = client.session.getState();
const ids: string[] = []; const caches: QueryClient[] = [];
beforeEach(() => {
  vi.stubGlobal('crypto', webcrypto);
  wire.draft = threadDraft([{ type: 'text', text: 'My unsent question' }]);
  wire.load.mockReset().mockImplementation(() => Promise.resolve(wire.draft));
  wire.save.mockReset().mockImplementation((_id, content, delivery, comments) => {
    wire.draft = threadDraft(content, comments, { delivery, native_action: wire.draft?.native_action ?? null }); return Promise.resolve(wire.draft);
  });
  wire.send.mockReset(); wire.settings = []; useComposerStore.setState({ planning: {} });
});
afterEach(async () => { cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); for (const cache of caches.splice(0)) cache.clear(); client.session.setState(savedSession); vi.restoreAllMocks(); await setLocale('en'); });

function draw(layout: 'one' | 'two' = 'one', tray = true, plans = false) {
  const source = snapshot(null); const id = source.thread.id; ids.push(id);
  client.session.setState({ status: 'signed_in', userId: source.thread.owner.id, readerVersion: 1 });
  if (plans && source.thread.binding !== null) source.thread = { ...source.thread, usage: null, current_turn_id: null, binding: { ...source.thread.binding, plan_mode: true } };
  threadWriter.reset(id, source, false);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  wire.send.mockResolvedValue({ input: { ...threadInput('review-input', 'Sent review', { author: source.thread.owner, position: 'a' }), thread_id: id }, seq: source.seq + 1 });
  const content = (diff = DIFF) => <QueryClientProvider client={cache}><ReviewableDiff threadId={id} path="web/check.ts" itemId="recorded-change" diff={diff} layout={layout} label="Recorded difference" />{tray && <ReviewTray threadId={id} />}</QueryClientProvider>;
  return { ...show(content()), id, cache, content };
}
async function openLine(side: 'old' | 'new') {
  await userEvent.click(screen.getByRole('button', { name: i18n.t('source.review.commentLine', { line: 10, side: i18n.t(`source.review.side.${side}`) }) }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('source.review.title', { path: 'web/check.ts' }) });
  expect(within(dialog).getByRole('spinbutton', { name: i18n.t('source.review.from') })).toHaveValue(10);
  expect(within(dialog).getByRole('spinbutton', { name: i18n.t('source.review.to') })).toHaveValue(10);
  await userEvent.type(within(dialog).getByRole('textbox', { name: i18n.t('source.review.text') }), 'Please keep the check.');
  return dialog;
}

for (const locale of ['en', 'zh-CN'] as const) for (const layout of ['one', 'two'] as const) {
  it(`keeps the exact line, side and original hunk and sends one review in ${locale}/${layout}`, async () => {
    await setLocale(locale); vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(1200);
    const view = draw(layout);
    if (layout === 'two') await waitFor(() => expect(view.container.querySelector('table')).toHaveClass('table-fixed'));
    const side = layout === 'one' ? 'new' : 'old'; const dialog = await openLine(side);
    // A later live patch cannot move the comment that was opened on the earlier one.
    view.rerender(view.content(DIFF.replace('new condition', 'newer condition')));
    await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('source.review.save') }));
    await waitFor(() => expect(wire.draft?.review_comments).toHaveLength(1));
    const comment = wire.draft?.review_comments[0];
    expect(comment).toMatchObject({ path: 'web/check.ts', side, from_line: 10, to_line: 10, item_id: 'recorded-change', text: 'Please keep the check.',
      hunk: DIFF.trimEnd(), diff_hash: createHash('sha256').update(DIFF).digest('hex') });
    expect(wire.draft?.content).toEqual([{ type: 'text', text: 'My unsent question' }]);
    expect(await screen.findByText(i18n.t('source.review.waiting', { count: 1 }))).toBeVisible();
    const expected = reviewMessage(i18n.t, wire.draft?.review_comments ?? []);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('source.review.preview') }));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('source.review.send') }));
    await waitFor(() => expect(wire.send).toHaveBeenCalledTimes(1));
    const clientId = wire.send.mock.calls[0]?.[1].clientId;
    expect(clientId).toEqual(expect.any(String));
    expect(wire.send).toHaveBeenCalledWith(view.id, expect.objectContaining({ content: [{ type: 'text', text: expected }], delivery: 'queue', clientId }));
    await waitFor(() => expect(wire.draft?.review_comments).toEqual([]));
    expect(wire.draft?.content).toEqual([{ type: 'text', text: 'My unsent question' }]);
  });
}

it('retires the private form and never saves after the reader changes during its draft read', async () => {
  const pending = deferred<Draft | null>(); wire.load.mockReturnValue(pending.promise);
  const view = draw('one', false); const dialog = await openLine('new');
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('source.review.save') }));
  await waitFor(() => expect(wire.load).toHaveBeenCalledTimes(1));
  act(() => client.session.setState({ userId: 'another-reader', readerVersion: 2 }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await act(async () => { pending.resolve(threadDraft([{ type: 'text', text: 'Previous person’s private draft' }])); await pending.promise; });
  await waitFor(() => expect(view.cache.getMutationCache().getAll().every((entry) => entry.state.status !== 'pending')).toBe(true));
  expect(wire.save).not.toHaveBeenCalled(); expect(wire.send).not.toHaveBeenCalled();
});

it('does not publish a late saved comment into a later reader’s private cache', async () => {
  const saved = deferred<Draft>(); wire.save.mockReturnValue(saved.promise);
  const view = draw('one', false); const dialog = await openLine('new');
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('source.review.save') }));
  await waitFor(() => expect(wire.save).toHaveBeenCalledTimes(1));
  act(() => { client.session.setState({ userId: 'another-reader', readerVersion: 2 }); view.cache.clear(); });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await act(async () => { saved.resolve(threadDraft([{ type: 'text', text: 'Old private data' }])); await saved.promise; });
  expect(view.cache.getQueryData(threadKeys.draft(view.id))).toBeUndefined();
  expect(wire.send).not.toHaveBeenCalled();
});

for (const chosen of [undefined, false, true]) it(`sends review with the same Plan choice as the composer (${String(chosen)})`, async () => {
  wire.settings = [{ key: 'thread.plan_first', value: true, own: null, source: { scope: 'topic', node: null }, locked_by: null, writable: false, parts: [] }];
  const view = draw('one', true, true);
  if (chosen !== undefined) composerWriter.setPlanning(view.id, chosen);
  const dialog = await openLine('new');
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('source.review.save') }));
  await screen.findByText(i18n.t('source.review.waiting', { count: 1 }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('source.review.preview') }));
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('source.review.send') })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: i18n.t('source.review.send') }));
  await waitFor(() => expect(wire.send).toHaveBeenCalledExactlyOnceWith(view.id, expect.objectContaining({ plan: chosen ?? true })));
});
