/** Inherited passive hooks share one explicit owner; imperative work never infers a route selector. */
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { fetchItems, fetchThreadSync } from '@/api/threads';
import { feeds, refreshThread, socket, useThreadFeed } from '@/realtime/live';
import { ConnectionBanner } from '@/features/thread/ConnectionBanner';
import { i18n, setLocale } from '@/i18n';
import { ThreadReadProvider, useThreadReadRef } from '@/realtime/threadReadContext';
import { defaultThreadRead, resultThreadRead, sameThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';
import { flushDeltas, threadViewOf, threadWriter, useInput, useItem, useThreadPart, useThreadRow, useThreadView } from '@/realtime/stores/thread';
import { answer } from '../unit/continuation-fixtures';
import { A, B, OTHER, OLDER, SHARED, T, bodyOf, serve } from '../unit/thread-read-fixtures';
import { show } from './support';

const route = defaultThreadRead(T), preview = resultThreadRead(T,'context-preview'), second = resultThreadRead(T,'context-second');
function Seen({ target,label }: { target: ThreadReadTarget; label: string }) {
  const view = useThreadView(target), ref = useThreadReadRef(target), row = useItem(target,SHARED);
  const count = useThreadPart(target,(value) => value.order.length), thread = useThreadRow(target), input = useInput(target,'not-held');
  return <output aria-label={label}>{bodyOf(row)}|{count}|{ref.threadId}|{ref.key.kind}|{thread?.id}|{view.closed ?? 'open'}|{input === undefined ? 'no-input' : input.id}</output>;
}
async function open(target: typeof route,selection: string | null) {
  selectThreadContinuation(target,selection); await fetchThreadSync(target,null);
}

beforeEach(() => { vi.useFakeTimers(); feeds.clear(); threadWriter.clear(); queryClient.clear(); });
afterEach(async () => { cleanup(); feeds.clear(); socket.stop(); flushDeltas(); threadWriter.clear(); queryClient.clear(); vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllTimers(); vi.useRealTimers(); await setLocale('en'); });

function ConnectionWindow({ label }: { label: string }) {
  useThreadFeed(T);
  return <section aria-label={label}><ConnectionBanner threadId={T} /></section>;
}

for (const locale of ['en', 'zh-CN'] as const) it(`retries only the failed window whose connection banner was pressed, even with the same thread and history in ${locale}`, async () => {
  await setLocale(locale);
  const server = serve((request) => new URL(request.url).pathname.endsWith('/sync') ? Promise.resolve(Response.json({
    type: 'about:blank', title: 'Read failed', status: 500, code: 'internal', detail: 'The current read failed', request_id: 'synthetic',
  }, { status: 500 })) : undefined);
  await open(route, B); await open(preview, B);
  show(<><ThreadReadProvider read={route}><ConnectionWindow label="Other window" /></ThreadReadProvider>
    <ThreadReadProvider read={preview}><ConnectionWindow label="Retry window" /></ThreadReadProvider></>);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  const page = await fetchItems(route, { around: OLDER });
  act(() => threadWriter.window(route, page.items, false, true));
  await act(async () => { refreshThread(route); refreshThread(preview); await vi.advanceTimersByTimeAsync(0); });
  expect(threadViewOf(route).error).not.toBeNull();
  expect(threadViewOf(preview).error).not.toBeNull();
  const other = threadViewOf(route), count = server.calls.length;
  const retry = within(screen.getByRole('region', { name: 'Retry window' })).getByRole('button', { name: i18n.t('thread.connection.retry') });
  await act(async () => { fireEvent.click(retry); await vi.advanceTimersByTimeAsync(0); });
  expect(threadViewOf(preview).error).toBeNull();
  expect(threadViewOf(preview).order).toEqual([SHARED]);
  expect(threadViewOf(route)).toBe(other);
  expect(threadViewOf(route).order).toEqual([OLDER]);
  expect(server.calls.slice(count)).toHaveLength(1);
  const request = new URL(server.calls[count]!.url);
  expect(request.pathname).toBe(`/api/v1/threads/${T}/opening`);
  expect(request.searchParams.get('continuation_id')).toBe(B);
  expect(within(screen.getByRole('region', { name: 'Other window' })).getByRole('button', { name: i18n.t('thread.connection.retry') })).toBeVisible();
  expect(within(screen.getByRole('region', { name: 'Retry window' })).queryByRole('button')).not.toBeInTheDocument();
});

it('stops offering a stale connection Retry after that read has ended', async () => {
  const server = serve(); await open(preview, B);
  threadWriter.failed(preview, new Error('The last read failed'));
  show(<ThreadReadProvider read={preview}><ConnectionBanner threadId={T} /></ThreadReadProvider>);
  expect(screen.getByRole('button', { name: i18n.t('thread.connection.retry') })).toBeVisible();
  act(() => threadWriter.closed(preview, 'revoked'));
  expect(screen.queryByRole('button', { name: i18n.t('thread.connection.retry') })).not.toBeInTheDocument();
  await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
  expect(threadViewOf(preview).closed).toBe('revoked');
  expect(server.calls).toHaveLength(1);
});

it('matching-thread hooks inherit the provider, explicit default refs win, and other thread lookups remain default', async () => {
  serve(); await open(route,null); await open(preview,B); await open(defaultThreadRead(OTHER),null);
  render(<ThreadReadProvider read={preview}><Seen target={T} label="inherited" /><Seen target={route} label="explicit" /><Seen target={OTHER} label="other" /></ThreadReadProvider>);
  expect(screen.getByLabelText('inherited')).toHaveTextContent(`body-${B}|1|${T}|result|${T}|open|no-input`);
  expect(screen.getByLabelText('explicit')).toHaveTextContent(`body-${A}|1|${T}|thread|${T}|open|no-input`);
  expect(screen.getByLabelText('other')).toHaveTextContent(`body-${A}|1|${OTHER}|thread|${OTHER}|open|no-input`);
  act(() => threadWriter.apply(preview,{ items: [answer('preview-own-suffix','New own row',B,25)] },captureThreadRead(preview)));
  expect(screen.getByLabelText('inherited')).toHaveTextContent(`body-${B}|2`);
  expect(screen.getByLabelText('explicit')).toHaveTextContent(`body-${A}|1`);
});

it('imperative callbacks use an explicit inherited ref while native string calls still address the route', async () => {
  serve(); await open(route,null); await open(preview,B); const before = threadViewOf(route);
  let routeWasDefault = false;
  function OpenWindow() {
    const ref = useThreadReadRef(T);
    return <button onClick={async () => {
      routeWasDefault = sameThreadRead(captureThreadRead(T).ref,route);
      const page = await fetchItems(ref,{ around: OLDER }); threadWriter.window(ref,page.items,page.has_older,page.has_newer);
    }}>Open window</button>;
  }
  render(<ThreadReadProvider read={preview}><OpenWindow /><Seen target={T} label="preview" /></ThreadReadProvider>);
  await act(async () => { fireEvent.click(screen.getByRole('button',{ name: 'Open window' })); await vi.advanceTimersByTimeAsync(0); });
  expect(routeWasDefault).toBe(true); expect(threadViewOf(preview).order).toEqual([OLDER]);
  expect(threadViewOf(route)).toBe(before); expect(screen.getByLabelText('preview')).toHaveTextContent(`|${T}|result|`);
});

it('two same-thread providers and replacement context values never share or retain the wrong local window', async () => {
  serve(); await open(preview,B); await open(second,B);
  const page = await fetchItems(preview,{ around: OLDER }); threadWriter.window(preview,page.items,false,true);
  const view = render(<div><ThreadReadProvider read={preview}><Seen target={T} label="left" /></ThreadReadProvider><ThreadReadProvider read={second}><Seen target={T} label="right" /></ThreadReadProvider></div>);
  expect(screen.getByLabelText('left')).toHaveTextContent('|1|'); expect(screen.getByLabelText('left')).not.toHaveTextContent(`body-${B}`);
  expect(screen.getByLabelText('right')).toHaveTextContent(`body-${B}`);
  view.rerender(<ThreadReadProvider read={second}><Seen target={T} label="left" /></ThreadReadProvider>);
  expect(screen.getByLabelText('left')).toHaveTextContent(`body-${B}`);
});

it('actual feed hooks aggregate visibility by native thread while each owner can hide independently', async () => {
  const server = serve(); vi.spyOn(document,'visibilityState','get').mockReturnValue('visible');
  const visible = vi.spyOn(socket,'visible');
  selectThreadContinuation(preview,B);
  function Feeds({ routeShown,previewShown }: { routeShown: boolean; previewShown: boolean }) {
    useThreadFeed(routeShown ? route : null); useThreadFeed(previewShown ? preview : null);
    return null;
  }
  const view = render(<Feeds routeShown previewShown />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(2);
  expect(visible).toHaveBeenLastCalledWith(`thread:${T}`,true);
  view.rerender(<Feeds routeShown previewShown={false} />);
  expect(visible).toHaveBeenLastCalledWith(`thread:${T}`,true);
  view.rerender(<Feeds routeShown={false} previewShown={false} />);
  expect(visible).toHaveBeenLastCalledWith(`thread:${T}`,false);
  expect(visible.mock.calls.every(([feed]) => feed === `thread:${T}`)).toBe(true);
});
